#!/usr/bin/env python3
"""Build Laurel's small ffmpeg.exe and stage it into the OBS runtime.

Laurel trims clips and draws thumbnails with FFmpeg's command-line program. The OBS
runtime (tools/obs-runtime.py) already carries the FFmpeg 7.1.1 libraries that OBS
ships (built by obs-deps with --enable-gpl --enable-version3), so Laurel builds only
the program, from the official FFmpeg 7.1.1 release, and links it to those DLLs:
about 0.3 MB instead of a second copy of FFmpeg.

The build runs in WSL (decision D10): clang-cl and lld-link with the MSVC CRT and the
Windows SDK that cargo-xwin downloads. Only fftools is compiled; the import libraries
come from the exports of the DLLs in the pinned OBS zip. The program uses the
dynamic CRT (-MD), like the OBS libraries. Paths reach the compiler through INCLUDE
and LIB, so the configuration string inside ffmpeg.exe holds no local paths.

  <dest>/bin/64bit/ffmpeg.exe     next to the FFmpeg DLLs of the runtime
  <dest>/licenses/FFMPEG.txt      what was built, from what, and the source
  <dest>/licenses/gplv3.txt       the license of this FFmpeg build

Usage:
  python3 -I tools/ffmpeg-build.py build          build if needed; prints the exe path
  python3 -I tools/ffmpeg-build.py stage <dest>   build if needed, then copy into <dest>
"""

import hashlib
import importlib.util
import os
import shutil
import subprocess
import sys
import tarfile
import tempfile
import urllib.request
import zipfile
from pathlib import Path

VERSION = "7.1.1"
TAR_NAME = f"ffmpeg-{VERSION}.tar.xz"
TAR_URL = f"https://ffmpeg.org/releases/{TAR_NAME}"
# Checked against the release signature (FFmpeg release signing key,
# FCF9 86EA 15E6 E293 A564 4F10 B432 2F04 D676 58D8) when it was pinned.
TAR_SHA256 = "733984395e0dbbe5c046abda2dc49a5544e7e0e1e2366bba849222ae9e3a03b1"

# The FFmpeg DLLs of the OBS runtime, by base name of their import library.
DLLS = {
    "avdevice": "avdevice-61.dll",
    "avfilter": "avfilter-10.dll",
    "avformat": "avformat-61.dll",
    "avcodec": "avcodec-61.dll",
    "swresample": "swresample-5.dll",
    "swscale": "swscale-8.dll",
    "avutil": "avutil-59.dll",
}
# The version string avutil-59.dll of the OBS runtime reports; the program must be
# built from the same release.
OBS_FFMPEG_VERSION = b"n7.1.1\0"

CONFIGURE = [
    "--enable-cross-compile", "--target-os=win64", "--arch=x86_64", "--toolchain=msvc",
    "--cc=clang-cl", "--cxx=clang-cl", "--ld=lld-link", "--ar=llvm-lib", "--nm=llvm-nm", "--host-cc=gcc",
    "--extra-cflags=--target=x86_64-pc-windows-msvc -MD -D_WIN32_WINNT=0x0A00 -Brepro",
    "--disable-x86asm", "--enable-gpl", "--enable-version3", "--disable-postproc",
    "--enable-shared", "--disable-static", "--disable-doc", "--disable-debug",
    "--disable-ffplay", "--disable-ffprobe",
]
OBJS = [
    "ffmpeg", "ffmpeg_dec", "ffmpeg_demux", "ffmpeg_enc", "ffmpeg_filter", "ffmpeg_hw", "ffmpeg_mux",
    "ffmpeg_mux_init", "ffmpeg_opt", "ffmpeg_sched", "objpool", "sync_queue", "thread_queue",
    "cmdutils", "opt_common",
]
# EXTRALIBS-ffmpeg in the generated ffbuild/config.mak.
SYSTEM_LIBS = ["ole32.lib", "psapi.lib", "shell32.lib"]


def log(msg: str) -> None:
    print(f"[ffmpeg-build] {msg}", file=sys.stderr)


def cache_dir() -> Path:
    base = os.environ.get("XDG_CACHE_HOME") or str(Path.home() / ".cache")
    return Path(base) / "laurel" / "ffmpeg"


def xwin_dir() -> Path:
    base = os.environ.get("XDG_CACHE_HOME") or str(Path.home() / ".cache")
    path = Path(base) / "cargo-xwin" / "xwin"
    if not (path / "crt" / "include").is_dir():
        sys.exit(f"[ffmpeg-build] no MSVC CRT in {path}: run a cargo xwin build once (tools/winrun.sh)")
    return path


def sha256_bytes(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def sha256_file(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as f:
        for block in iter(lambda: f.read(1 << 20), b""):
            h.update(block)
    return h.hexdigest()


def obs_runtime():
    """tools/obs-runtime.py as a module, for the pinned OBS zip."""
    spec = importlib.util.spec_from_file_location("obs_runtime", Path(__file__).with_name("obs-runtime.py"))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def fetch_source() -> Path:
    """The extracted source tree, downloading and checking the release first if needed."""
    tar = cache_dir() / TAR_NAME
    if not tar.exists():
        tar.parent.mkdir(parents=True, exist_ok=True)
        log(f"downloading {TAR_URL}")
        with tempfile.NamedTemporaryFile(dir=tar.parent, delete=False) as tmp:
            with urllib.request.urlopen(TAR_URL) as r:
                for block in iter(lambda: r.read(1 << 20), b""):
                    tmp.write(block)
        os.replace(tmp.name, tar)
    actual = sha256_file(tar)
    if actual != TAR_SHA256:
        sys.exit(f"[ffmpeg-build] {tar}: SHA-256 {actual} does not match the pinned {TAR_SHA256}")
    src = cache_dir() / "src" / f"ffmpeg-{VERSION}"
    if not (src / "configure").exists():
        shutil.rmtree(src, ignore_errors=True)
        src.parent.mkdir(parents=True, exist_ok=True)
        with tarfile.open(tar) as t:
            t.extractall(src.parent, filter="data")
    return src


def obs_dlls() -> dict[str, bytes]:
    """The FFmpeg DLLs from the pinned OBS zip, checked to be FFmpeg {VERSION}."""
    rt = obs_runtime()
    with zipfile.ZipFile(rt.fetch()) as z:
        dlls = {name: z.read(f"bin/64bit/{dll}") for name, dll in DLLS.items()}
    if OBS_FFMPEG_VERSION not in dlls["avutil"]:
        sys.exit(f"[ffmpeg-build] the FFmpeg libraries of OBS {rt.VERSION} are not FFmpeg {VERSION}: update VERSION and TAR_SHA256")
    return dlls


def run(cmd: list[str], cwd: Path, env: dict[str, str]) -> None:
    result = subprocess.run(cmd, cwd=cwd, env=env, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
    if result.returncode != 0:
        tail = "\n".join(result.stdout.splitlines()[-30:])
        sys.exit(f"[ffmpeg-build] {' '.join(cmd[:3])} … failed:\n{tail}")


def stamp(dlls: dict[str, bytes]) -> str:
    """What the build depends on; a different stamp rebuilds."""
    parts = [TAR_SHA256, *CONFIGURE, *OBJS, *SYSTEM_LIBS] + [f"{n}={sha256_bytes(d)}" for n, d in sorted(dlls.items())]
    return sha256_bytes("\n".join(parts).encode())


def build() -> Path:
    """Builds ffmpeg.exe if it is missing or out of date, and returns its path."""
    dlls = obs_dlls()
    out_dir = cache_dir() / f"ffmpeg-{VERSION}"
    exe = out_dir / "ffmpeg.exe"
    stamp_file = out_dir / "build.stamp"
    want = stamp(dlls)
    if exe.exists() and stamp_file.exists() and stamp_file.read_text() == want:
        return exe
    src = fetch_source()
    xwin = xwin_dir()
    env = dict(os.environ)
    env["INCLUDE"] = ";".join(str(xwin / p) for p in ["crt/include", "sdk/include/ucrt", "sdk/include/um", "sdk/include/shared"])
    env["LIB"] = ";".join(str(xwin / p) for p in ["crt/lib/x86_64", "sdk/lib/um/x86_64", "sdk/lib/ucrt/x86_64"])
    build_dir = cache_dir() / "build"
    shutil.rmtree(build_dir, ignore_errors=True)
    shutil.rmtree(out_dir, ignore_errors=True)
    build_dir.mkdir(parents=True)
    out_dir.mkdir(parents=True)
    log(f"configuring FFmpeg {VERSION}")
    run([str(src / "configure"), *CONFIGURE], build_dir, env)
    log("compiling fftools")
    run(["make", "libavutil/ffversion.h"], build_dir, env)
    run(["make", f"-j{os.cpu_count() or 4}", *[f"fftools/{o}.o" for o in OBJS]], build_dir, env)
    implib = build_dir / "implib"
    implib.mkdir()
    for name, data in dlls.items():
        dll = implib / DLLS[name]
        dll.write_bytes(data)
        exports = subprocess.run(["llvm-readobj", "--coff-exports", str(dll)], capture_output=True, text=True, check=True).stdout
        symbols = [line.split("Name:", 1)[1].strip() for line in exports.splitlines() if line.strip().startswith("Name:")]
        (implib / f"{name}.def").write_text("\n".join([f"LIBRARY {DLLS[name]}", "EXPORTS", *symbols]) + "\n")
        run(["llvm-lib", "-nologo", f"-def:{name}.def", "-machine:x64", f"-out:{name}.lib"], implib, env)
    log("linking ffmpeg.exe")
    run(
        ["lld-link", "-nologo", f"-out:{exe}", "-subsystem:console", "-machine:x64", "-opt:ref", "-opt:icf", "-Brepro", f"-libpath:{implib}"]
        + [f"fftools/{o}.o" for o in OBJS]
        + [f"{name}.lib" for name in DLLS]
        + SYSTEM_LIBS,
        build_dir,
        env,
    )
    data = exe.read_bytes()
    for private in [str(Path.home()).encode(), str(cache_dir()).encode()]:
        if private in data:
            sys.exit(f"[ffmpeg-build] {exe} holds a local path ({private.decode()}); it must not be shipped")
    shutil.copyfile(src / "COPYING.GPLv3", out_dir / "gplv3.txt")
    stamp_file.write_text(want)
    log(f"built {exe} ({len(data)} bytes, SHA-256 {sha256_bytes(data)})")
    return exe


def notice(exe: Path) -> str:
    rt = obs_runtime()
    flags = "\n".join(f"    {f}" for f in CONFIGURE)
    return f"""Laurel's ffmpeg.exe

ffmpeg.exe is FFmpeg's own command-line program, version {VERSION}, unmodified,
built by Laurel's tools/ffmpeg-build.py from the official release:
  {TAR_URL}
  SHA-256 {TAR_SHA256}
with these configure options (only the program is compiled):
{flags}
It uses the FFmpeg {VERSION} libraries of this runtime (avcodec, avdevice, avfilter,
avformat, avutil, swresample, swscale), which come with OBS Studio {rt.VERSION}
(see OBS-RUNTIME.txt; obs-deps {rt.OBS_DEPS_VERSION} builds them with --enable-gpl and
--enable-version3).

ffmpeg.exe SHA-256 {sha256_file(exe)}

This FFmpeg build is free software under the GNU General Public License, version 3
or later (gplv3.txt in this folder). Corresponding source code: the release above,
and tools/ffmpeg-build.py in Laurel's source code; each Laurel release also carries
the release archive in Laurel-<version>-sources.zip.
"""


def stage(dest: Path) -> None:
    exe = build()
    target = dest / "bin" / "64bit" / "ffmpeg.exe"
    if not target.parent.is_dir():
        sys.exit(f"[ffmpeg-build] {target.parent} does not exist: stage the OBS runtime first (tools/obs-runtime.py)")
    files = {
        target: exe.read_bytes(),
        dest / "licenses" / "FFMPEG.txt": notice(exe).replace("\n", "\r\n").encode(),
        dest / "licenses" / "gplv3.txt": (exe.parent / "gplv3.txt").read_bytes(),
    }
    changed = 0
    for path, data in files.items():
        if not path.exists() or path.read_bytes() != data:
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(data)
            changed += 1
    log(f"{target}: {'staged' if changed else 'up to date'} (FFmpeg {VERSION})")


def main(argv: list[str]) -> int:
    if len(argv) == 2 and argv[1] == "build":
        print(build())
        return 0
    if len(argv) == 3 and argv[1] == "stage":
        stage(Path(argv[2]))
        return 0
    print(__doc__, file=sys.stderr)
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv))
