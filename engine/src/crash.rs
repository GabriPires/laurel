//! A19 of the stability tests: an engine that dies of an exception nothing handles
//! leaves the cause in its log. The filter writes the exception code, where it
//! happened (module and offset), the thread, the crashing thread's stack (module
//! and offset of each return address) and the thread's name to stderr, which the
//! app writes to engine.log, then lets Windows end the process as before (Windows
//! Error Reporting; the exit code is the exception code). It formats into stack
//! buffers and writes with WriteFile, without the heap, since a damaged heap may be
//! what failed; only the thread name, read last, needs Windows to allocate.
//!
//! Not covered: a fast fail (an abort, a failed buffer check, heap corruption caught
//! by Windows) ends the process without calling any filter. For those, and for
//! every other end, the app writes the engine's exit code to engine.log.

use std::ffi::c_void;
use std::fmt::Write as _;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

use windows::Win32::System::Diagnostics::Debug::{
    RtlLookupFunctionEntry, RtlVirtualUnwind, SetUnhandledExceptionFilter, CONTEXT, EXCEPTION_CONTINUE_SEARCH, EXCEPTION_POINTERS,
    LPTOP_LEVEL_EXCEPTION_FILTER, UNW_FLAG_NHANDLER,
};

#[link(name = "kernel32", kind = "raw-dylib")]
extern "system" {
    fn GetModuleHandleExW(flags: u32, name: *const u16, module: *mut isize) -> i32;
    fn GetModuleFileNameW(module: isize, file: *mut u16, size: u32) -> u32;
    fn GetCurrentThreadId() -> u32;
    fn GetCurrentThread() -> isize;
    fn GetCurrentThreadStackLimits(low: *mut usize, high: *mut usize);
    fn GetThreadDescription(thread: isize, description: *mut *mut u16) -> i32;
    fn LocalFree(memory: *mut c_void) -> *mut c_void;
    fn GetStdHandle(which: u32) -> isize;
    fn WriteFile(file: isize, buffer: *const u8, length: u32, written: *mut u32, overlapped: *mut c_void) -> i32;
    fn GetCurrentProcess() -> isize;
    fn TerminateProcess(process: isize, code: u32) -> i32;
}

const STD_ERROR_HANDLE: u32 = -12i32 as u32;
const GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT: u32 = 0x2;
const GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS: u32 = 0x4;
const EXCEPTION_ACCESS_VIOLATION: u32 = 0xC000_0005;
const EXCEPTION_IN_PAGE_ERROR: u32 = 0xC000_0006;
const EXCEPTION_STACK_OVERFLOW: u32 = 0xC000_00FD;
/// Return addresses written after the crash location.
const MAX_FRAMES: usize = 48;

/// The filter that was there before (the C runtime's), called after the report.
static PREVIOUS: OnceLock<LPTOP_LEVEL_EXCEPTION_FILTER> = OnceLock::new();
/// A second exception inside the report goes straight to the previous filter.
static INSIDE: AtomicBool = AtomicBool::new(false);
/// Set by `deliberate`: after the report, the process ends with the exception code
/// itself, without a Windows Error Reporting report of a test.
static DELIBERATE: AtomicBool = AtomicBool::new(false);

/// Installs the filter; the engine calls it first thing.
pub fn install() {
    let previous = unsafe { SetUnhandledExceptionFilter(Some(filter)) };
    let _ = PREVIOUS.set(previous);
}

/// Ends the process at once with `code`, without running anything else: for an
/// engine whose shutdown hangs (health.rs).
pub fn terminate(code: u32) -> ! {
    unsafe { TerminateProcess(GetCurrentProcess(), code) };
    std::process::abort()
}

/// Crashes on purpose, for the self-test of this report: an access violation
/// writing near address 0, as through a null pointer to a structure. The report is
/// the same as for a real crash.
pub fn deliberate() -> ! {
    DELIBERATE.store(true, Ordering::SeqCst);
    unsafe { std::ptr::write_volatile(std::ptr::without_provenance_mut::<u32>(8), 0x4C41_5552) };
    std::process::abort()
}

unsafe extern "system" fn filter(info: *const EXCEPTION_POINTERS) -> i32 {
    unsafe {
        if !info.is_null() && !(*info).ExceptionRecord.is_null() && !INSIDE.swap(true, Ordering::SeqCst) {
            report(&*info);
            if DELIBERATE.load(Ordering::SeqCst) {
                TerminateProcess(GetCurrentProcess(), (*(*info).ExceptionRecord).ExceptionCode.0 as u32);
            }
        }
        match PREVIOUS.get().copied().flatten() {
            Some(previous) => previous(info),
            None => EXCEPTION_CONTINUE_SEARCH,
        }
    }
}

/// The name of an exception code in Windows' headers (or what raises it).
fn name_of(code: u32) -> Option<&'static str> {
    Some(match code {
        EXCEPTION_ACCESS_VIOLATION => "EXCEPTION_ACCESS_VIOLATION",
        EXCEPTION_IN_PAGE_ERROR => "EXCEPTION_IN_PAGE_ERROR",
        0xC000_001D => "EXCEPTION_ILLEGAL_INSTRUCTION",
        0xC000_0094 => "EXCEPTION_INT_DIVIDE_BY_ZERO",
        0xC000_0096 => "EXCEPTION_PRIV_INSTRUCTION",
        EXCEPTION_STACK_OVERFLOW => "EXCEPTION_STACK_OVERFLOW",
        0xC000_0374 => "STATUS_HEAP_CORRUPTION",
        0x8000_0003 => "EXCEPTION_BREAKPOINT",
        0xE06D_7363 => "a C++ exception",
        _ => return None,
    })
}

unsafe fn report(info: &EXCEPTION_POINTERS) {
    unsafe {
        let record = &*info.ExceptionRecord;
        let code = record.ExceptionCode.0 as u32;
        let mut line = Line::new();
        let _ = write!(line, "[laurel] crash: unhandled exception 0x{code:08X}");
        if let Some(name) = name_of(code) {
            let _ = write!(line, " ({name})");
        }
        let _ = write!(line, " in thread {} at ", GetCurrentThreadId());
        write_location(&mut line, record.ExceptionAddress as u64);
        if matches!(code, EXCEPTION_ACCESS_VIOLATION | EXCEPTION_IN_PAGE_ERROR) && record.NumberParameters >= 2 {
            let access = match record.ExceptionInformation[0] {
                0 => "reading",
                1 => "writing",
                8 => "executing",
                _ => "accessing",
            };
            let _ = write!(line, ", {access} 0x{:016X}", record.ExceptionInformation[1]);
        }
        line.emit();
        // A stack overflow leaves too little stack for the rest.
        if code == EXCEPTION_STACK_OVERFLOW || info.ContextRecord.is_null() {
            return;
        }
        write_stack(&*info.ContextRecord);
        write_thread_name();
        let mut line = Line::new();
        let _ = write!(line, "[laurel] crash: end of report");
        line.emit();
    }
}

/// CONTEXT must be 16-byte aligned for RtlVirtualUnwind; the crate's is 8.
#[repr(C, align(16))]
struct Context(CONTEXT);

/// The return addresses of the crashing thread, unwound with the modules' x64
/// unwind data from the context of the exception.
unsafe fn write_stack(context: &CONTEXT) {
    unsafe {
        let mut ctx = Context(*context);
        let (mut low, mut high) = (0usize, 0usize);
        GetCurrentThreadStackLimits(&mut low, &mut high);
        let on_stack = |sp: u64| sp as usize >= low && (sp as usize).saturating_add(8) <= high;
        for frame in 0..MAX_FRAMES {
            let (pc, sp) = (ctx.0.Rip, ctx.0.Rsp);
            if !on_stack(sp) {
                break;
            }
            let mut base = 0u64;
            let entry = RtlLookupFunctionEntry(pc, &mut base, None);
            if entry.is_null() {
                // Only the crash location can be a leaf function (or a call through a
                // bad pointer): its return address is on top of the stack.
                if frame > 0 {
                    break;
                }
                ctx.0.Rip = *(sp as *const u64);
                ctx.0.Rsp = sp + 8;
            } else {
                let mut handler_data = std::ptr::null_mut();
                let mut establisher = 0u64;
                RtlVirtualUnwind(UNW_FLAG_NHANDLER, base, pc, entry, &mut ctx.0, &mut handler_data, &mut establisher, None);
            }
            if ctx.0.Rip == 0 || ctx.0.Rsp <= sp {
                break;
            }
            let mut line = Line::new();
            let _ = write!(line, "[laurel] crash: called from ");
            write_location(&mut line, ctx.0.Rip);
            line.emit();
        }
    }
}

/// The thread's name, as libobs and the engine set them ("libobs: graphics thread").
unsafe fn write_thread_name() {
    unsafe {
        let mut name: *mut u16 = std::ptr::null_mut();
        if GetThreadDescription(GetCurrentThread(), &mut name) < 0 || name.is_null() {
            return;
        }
        let mut len = 0;
        while len < 128 && *name.add(len) != 0 {
            len += 1;
        }
        if len > 0 {
            let mut line = Line::new();
            let _ = write!(line, "[laurel] crash: thread name: ");
            for c in char::decode_utf16(std::slice::from_raw_parts(name, len).iter().copied()) {
                line.push(c.unwrap_or('?'));
            }
            line.emit();
        }
        LocalFree(name.cast());
    }
}

/// `module.dll+0x1A2B` (the file name only, no folder), or the bare address
/// outside any module.
unsafe fn write_location(line: &mut Line, address: u64) {
    unsafe {
        let mut module = 0isize;
        let flags = GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT;
        if address == 0 || GetModuleHandleExW(flags, address as *const u16, &mut module) == 0 {
            let _ = write!(line, "0x{address:016X}");
            return;
        }
        let mut path = [0u16; 260];
        let n = GetModuleFileNameW(module, path.as_mut_ptr(), path.len() as u32) as usize;
        let path = &path[..n.min(path.len())];
        let start = path.iter().rposition(|&c| c == u16::from(b'\\')).map_or(0, |i| i + 1);
        if start == path.len() {
            line.push('?');
        }
        for c in char::decode_utf16(path[start..].iter().copied()) {
            line.push(c.unwrap_or('?'));
        }
        let _ = write!(line, "+0x{:X}", address.wrapping_sub(module as u64));
    }
}

/// One log line in a stack buffer, written to stderr in a single WriteFile; it is
/// cut at the end of the buffer, never inside a character (the app reads UTF-8).
struct Line {
    buf: [u8; 512],
    len: usize,
}

impl Line {
    fn new() -> Line {
        Line { buf: [0; 512], len: 0 }
    }

    fn push(&mut self, c: char) {
        let mut utf8 = [0u8; 4];
        let bytes = c.encode_utf8(&mut utf8).as_bytes();
        // One byte stays free for the line end.
        if self.len + bytes.len() < self.buf.len() {
            self.buf[self.len..self.len + bytes.len()].copy_from_slice(bytes);
            self.len += bytes.len();
        }
    }

    fn emit(mut self) {
        self.buf[self.len] = b'\n';
        let mut written = 0u32;
        unsafe {
            WriteFile(GetStdHandle(STD_ERROR_HANDLE), self.buf.as_ptr(), self.len as u32 + 1, &mut written, std::ptr::null_mut());
        }
    }
}

impl std::fmt::Write for Line {
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        for c in s.chars() {
            self.push(c);
        }
        Ok(())
    }
}
