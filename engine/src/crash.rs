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
//!
//! A38: the same reading of a stack serves the request watch (hang.rs), which
//! writes the stack of every thread of the engine when a request of the app has
//! run for 40 s (`write_all_stacks`).

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
    fn GetCurrentProcessId() -> u32;
    fn CreateToolhelp32Snapshot(flags: u32, process: u32) -> isize;
    fn Thread32First(snapshot: isize, entry: *mut ThreadEntry) -> i32;
    fn Thread32Next(snapshot: isize, entry: *mut ThreadEntry) -> i32;
    fn CloseHandle(handle: isize) -> i32;
    fn OpenThread(access: u32, inherit: i32, thread: u32) -> isize;
    fn SuspendThread(thread: isize) -> u32;
    fn ResumeThread(thread: isize) -> u32;
    fn GetThreadContext(thread: isize, context: *mut CONTEXT) -> i32;
    fn VirtualQuery(address: *const c_void, info: *mut MemoryInfo, length: usize) -> usize;
    fn ReadProcessMemory(process: isize, address: *const c_void, buffer: *mut c_void, size: usize, read: *mut usize) -> i32;
}

/// THREADENTRY32.
#[repr(C)]
#[derive(Default)]
struct ThreadEntry {
    size: u32,
    usage: u32,
    thread: u32,
    process: u32,
    base_priority: i32,
    delta_priority: i32,
    flags: u32,
}

/// MEMORY_BASIC_INFORMATION (64-bit).
#[repr(C)]
#[derive(Default)]
struct MemoryInfo {
    base: usize,
    allocation_base: usize,
    allocation_protect: u32,
    partition: u16,
    region_size: usize,
    state: u32,
    protect: u32,
    kind: u32,
}

const TH32CS_SNAPTHREAD: u32 = 0x4;
const THREAD_SUSPEND_RESUME: u32 = 0x2;
const THREAD_GET_CONTEXT: u32 = 0x8;
const THREAD_QUERY_LIMITED_INFORMATION: u32 = 0x800;
/// CONTEXT_CONTROL and CONTEXT_INTEGER of AMD64: the registers an unwind needs.
const CONTEXT_CONTROL_INTEGER: u32 = 0x0010_0003;
/// The most of another thread's stack copied for its unwind, and zeros after it:
/// the unwind of the last frame copied may read a little past it.
const STACK_COPY: usize = 1 << 20;
const STACK_SLACK: usize = 64 << 10;

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
        write_crashing_thread_name();
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
        unwind(&mut ctx, low..high, |pc| {
            let mut line = Line::new();
            let _ = write!(line, "[laurel] crash: called from ");
            write_location(&mut line, pc);
            line.emit();
        });
    }
}

/// Calls `frame` with each return address from `ctx` up, for at most MAX_FRAMES,
/// while the stack pointer stays in `stack`.
unsafe fn unwind(ctx: &mut Context, stack: std::ops::Range<usize>, mut frame: impl FnMut(u64)) {
    unsafe {
        let on_stack = |sp: u64| sp as usize >= stack.start && (sp as usize).saturating_add(8) <= stack.end;
        for depth in 0..MAX_FRAMES {
            let (pc, sp) = (ctx.0.Rip, ctx.0.Rsp);
            if !on_stack(sp) {
                break;
            }
            let mut base = 0u64;
            let entry = RtlLookupFunctionEntry(pc, &mut base, None);
            if entry.is_null() {
                // Only the first location can be a leaf function (or a call through
                // a bad pointer): its return address is on top of the stack.
                if depth > 0 {
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
            frame(ctx.0.Rip);
        }
    }
}

/// A38: writes the name and the stack of every other thread of the engine, one
/// line each, between a first line that says why and a last one. Each thread is
/// suspended only while its registers and its stack are copied, with nothing that
/// takes a lock (the thread may hold the one it would wait for); the unwind and
/// the writing come after it runs again.
pub fn write_all_stacks(why: &str) {
    let mut line = Line::new();
    let _ = write!(line, "[laurel] hang: {why}; the stacks of the engine's threads follow");
    line.emit();
    let mut copy = vec![0u64; (STACK_COPY + STACK_SLACK) / 8];
    let mut threads = 0;
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
        if snapshot != -1 {
            let (process, me) = (GetCurrentProcessId(), GetCurrentThreadId());
            let mut entry = ThreadEntry { size: size_of::<ThreadEntry>() as u32, ..Default::default() };
            let mut more = Thread32First(snapshot, &mut entry) != 0;
            while more {
                if entry.process == process && entry.thread != me && write_thread_stack(entry.thread, &mut copy) {
                    threads += 1;
                }
                more = Thread32Next(snapshot, &mut entry) != 0;
            }
            CloseHandle(snapshot);
        }
    }
    let mut line = Line::new();
    let _ = write!(line, "[laurel] hang: end of the stacks ({threads} threads)");
    line.emit();
}

/// One thread's line: `thread <id> "<name>": <where it is> <return address>...`.
unsafe fn write_thread_stack(id: u32, copy: &mut [u64]) -> bool {
    unsafe {
        let thread = OpenThread(THREAD_SUSPEND_RESUME | THREAD_GET_CONTEXT | THREAD_QUERY_LIMITED_INFORMATION, 0, id);
        if thread == 0 {
            return false;
        }
        let mut ctx: Context = std::mem::zeroed();
        ctx.0.ContextFlags.0 = CONTEXT_CONTROL_INTEGER;
        let mut copied = 0usize;
        let suspended = SuspendThread(thread) != u32::MAX;
        let got = suspended && GetThreadContext(thread, &mut ctx.0) != 0;
        if got {
            // The stack from the thread's stack pointer up to its base, the end of
            // the committed region that holds it.
            let sp = ctx.0.Rsp as usize;
            let mut info = MemoryInfo::default();
            if VirtualQuery(sp as *const c_void, &mut info, size_of::<MemoryInfo>()) != 0 {
                let len = (info.base + info.region_size).saturating_sub(sp).min(STACK_COPY) & !7;
                if ReadProcessMemory(GetCurrentProcess(), sp as *const c_void, copy.as_mut_ptr().cast(), len, &mut copied) == 0 {
                    copied = 0;
                }
            }
        }
        if suspended {
            ResumeThread(thread);
        }
        if !got {
            CloseHandle(thread);
            return false;
        }
        let mut line = Line::new();
        let _ = write!(line, "[laurel] hang: thread {id}");
        write_thread_name(&mut line, thread, " \"", "\"");
        let _ = write!(line, ": ");
        write_location(&mut line, ctx.0.Rip);
        CloseHandle(thread);
        // The unwind reads the copy: every value that pointed into the copied part
        // of the stack, in the registers and on the stack itself (saved frame
        // pointers), is moved to the same place in the copy.
        let (start, end) = (ctx.0.Rsp, ctx.0.Rsp + copied as u64);
        let moved = (copy.as_ptr() as u64).wrapping_sub(start);
        let relocate = |v: u64| if v >= start && v < end { v.wrapping_add(moved) } else { v };
        for word in &mut copy[..copied / 8] {
            *word = relocate(*word);
        }
        copy[copied / 8..].fill(0);
        let c = &mut ctx.0;
        for register in [&mut c.Rsp, &mut c.Rbp, &mut c.Rax, &mut c.Rbx, &mut c.Rcx, &mut c.Rdx, &mut c.Rsi, &mut c.Rdi, &mut c.R8, &mut c.R9, &mut c.R10, &mut c.R11, &mut c.R12, &mut c.R13, &mut c.R14, &mut c.R15] {
            *register = relocate(*register);
        }
        let base = copy.as_ptr() as usize;
        unwind(&mut ctx, base..base + copied, |pc| {
            line.push(' ');
            write_location(&mut line, pc);
        });
        line.emit();
        true
    }
}

/// The crashing thread's name, as libobs and the engine set them ("libobs:
/// graphics thread").
unsafe fn write_crashing_thread_name() {
    unsafe {
        let mut line = Line::new();
        let _ = write!(line, "[laurel] crash: thread name: ");
        if write_thread_name(&mut line, GetCurrentThread(), "", "") {
            line.emit();
        }
    }
}

/// The name of a thread that has one, between `before` and `after`; whether it
/// has one.
unsafe fn write_thread_name(line: &mut Line, thread: isize, before: &str, after: &str) -> bool {
    unsafe {
        let mut name: *mut u16 = std::ptr::null_mut();
        if GetThreadDescription(thread, &mut name) < 0 || name.is_null() {
            return false;
        }
        let mut len = 0;
        while len < 128 && *name.add(len) != 0 {
            len += 1;
        }
        if len > 0 {
            let _ = line.write_str(before);
            for c in char::decode_utf16(std::slice::from_raw_parts(name, len).iter().copied()) {
                line.push(c.unwrap_or('?'));
            }
            let _ = line.write_str(after);
        }
        LocalFree(name.cast());
        len > 0
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
/// A38: long enough for a thread's whole stack on one line.
struct Line {
    buf: [u8; 2048],
    len: usize,
}

impl Line {
    fn new() -> Line {
        Line { buf: [0; 2048], len: 0 }
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
