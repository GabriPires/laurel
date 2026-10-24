#!/usr/bin/env python3
"""Fetch the pinned official OBS Studio build and stage Laurel's OBS runtime.

Laurel ships the official, signed OBS binaries instead of building its own: anti-cheats
accept OBS's signed game capture hook. Only the files the engine needs are staged, in
OBS's own folder layout:

  <dest>/bin/64bit/          libobs, its dependencies and the helper executables
                             (laurel-engine.exe goes here too)
  <dest>/obs-plugins/64bit/  the plugins loaded by the engine (MODULES in engine/src/main.rs)
  <dest>/data/               libobs effects and the plugins' data
  <dest>/licenses/           the GPL text and the source notice
  <dest>/obs-runtime.sha256  SHA-256 of every staged file

The layout matters: win-capture copies its hook to ProgramData (needed for Vulkan
games) from ..\\..\\data\\obs-plugins\\win-capture relative to the working directory,
which must be bin\\64bit.

Usage:
  python3 -I tools/obs-runtime.py fetch           download the zip and check its SHA-256
  python3 -I tools/obs-runtime.py stage <dest>    fetch if needed, then stage into <dest>
  python3 -I tools/obs-runtime.py verify <dest>   check the staged files against the manifest
"""

import hashlib
import os
import sys
import tempfile
import urllib.request
import zipfile
from pathlib import Path

VERSION = "32.1.2"
ZIP_NAME = f"OBS-Studio-{VERSION}-Windows-x64.zip"
ZIP_URL = f"https://github.com/obsproject/obs-studio/releases/download/{VERSION}/{ZIP_NAME}"
ZIP_SHA256 = "8d97e4563bd8d22d03e63042aa7dccede1d555c9bd35ce8a9e5019b0d0201bf6"
SOURCES_NAME = f"OBS-Studio-{VERSION}-Sources.tar.gz"
SOURCES_SHA256 = "c6532380c68a75327fe8b551461adeca8f184dcbe4015096251a6de76362a554"
# Pre-built dependencies used by this OBS release (CMakePresets.json at the tag).
OBS_DEPS_VERSION = "2025-08-23"

MODULES = ["win-capture", "win-wasapi", "obs-ffmpeg", "obs-x264", "obs-nvenc", "obs-qsv11"]

# libobs and the DLLs it and the modules import, plus the helpers that libobs and the
# encoder modules start from the executable's folder.
BIN = [
    "obs.dll", "libobs-d3d11.dll", "libobs-winrt.dll", "w32-pthreads.dll",
    "avcodec-61.dll", "avdevice-61.dll", "avfilter-10.dll", "avformat-61.dll", "avutil-59.dll",
    "swresample-5.dll", "swscale-8.dll", "libx264-164.dll", "librist.dll", "srt.dll",
    "libcurl.dll", "zlib.dll",
    "obs-ffmpeg-mux.exe", "obs-nvenc-test.exe", "obs-amf-test.exe", "obs-qsv-test.exe",
]

MANIFEST = "obs-runtime.sha256"
NOTICE = "licenses/OBS-RUNTIME.txt"
LICENSE_SRC = "data/obs-studio/license/gplv2.txt"


def cache_dir() -> Path:
    base = os.environ.get("XDG_CACHE_HOME") or str(Path.home() / ".cache")
    return Path(base) / "laurel" / "obs"


def sha256_file(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as f:
        for block in iter(lambda: f.read(1 << 20), b""):
            h.update(block)
    return h.hexdigest()


def fetch() -> Path:
    """Returns the cached zip, downloading it first if needed; always checks the hash."""
    path = cache_dir() / ZIP_NAME
    if not path.exists():
        path.parent.mkdir(parents=True, exist_ok=True)
        print(f"[obs-runtime] downloading {ZIP_URL}", file=sys.stderr)
        with tempfile.NamedTemporaryFile(dir=path.parent, delete=False) as tmp:
            with urllib.request.urlopen(ZIP_URL) as r:
                for block in iter(lambda: r.read(1 << 20), b""):
                    tmp.write(block)
        os.replace(tmp.name, path)
    actual = sha256_file(path)
    if actual != ZIP_SHA256:
        sys.exit(f"[obs-runtime] {path}: SHA-256 {actual} does not match the pinned {ZIP_SHA256}")
    return path


def wanted(name: str) -> bool:
    """Whether a zip entry belongs to the runtime."""
    if name.endswith("/"):
        return False
    if name.startswith("bin/64bit/"):
        return name[len("bin/64bit/"):] in BIN
    if name.startswith("obs-plugins/64bit/"):
        return name[len("obs-plugins/64bit/"):] in [f"{m}.dll" for m in MODULES]
    if name.startswith("data/libobs/"):
        return True
    for m in MODULES:
        prefix = f"data/obs-plugins/{m}/"
        if name.startswith(prefix):
            rest = name[len(prefix):]
            if rest.startswith("locale/"):
                return rest == "locale/en-US.ini"
            return not rest.endswith(".pdb")
    return False


def notice() -> str:
    return f"""Laurel OBS runtime

This folder holds unmodified files from the official OBS Studio {VERSION} build for
Windows x64:
  {ZIP_URL}
  SHA-256 {ZIP_SHA256}
{MANIFEST} lists every file with its SHA-256.

OBS Studio is free software under the GNU General Public License, version 2 or
later (gplv2.txt in this folder). Corresponding source code:
  OBS Studio {VERSION}: https://github.com/obsproject/obs-studio/tree/{VERSION}
    and {SOURCES_NAME} (SHA-256 {SOURCES_SHA256})
    in https://github.com/obsproject/obs-studio/releases/tag/{VERSION}
  Pre-built dependencies of this release (FFmpeg, x264, libcurl, librist, SRT, zlib):
    obs-deps {OBS_DEPS_VERSION}, https://github.com/obsproject/obs-deps/releases/tag/{OBS_DEPS_VERSION}
Each Laurel release also carries all of this source, with the libraries obs-deps
builds into these files, in Laurel-<version>-sources.zip (tools/gpl-sources.py).
"""


def read_manifest(dest: Path) -> dict[str, str]:
    path = dest / MANIFEST
    if not path.exists():
        return {}
    entries = {}
    for line in path.read_text().splitlines():
        if line and not line.startswith("#"):
            digest, name = line.split("  ", 1)
            entries[name] = digest
    return entries


def stamp() -> str:
    # The notice's hash too, so a staged runtime gets a changed notice.
    return f"# OBS Studio {VERSION} {ZIP_SHA256} notice {hashlib.sha256(notice().encode()).hexdigest()[:12]}"


def stage(dest: Path) -> None:
    manifest_path = dest / MANIFEST
    if manifest_path.exists() and manifest_path.read_text().startswith(stamp() + "\n") and not verify(dest, quiet=True):
        print(f"[obs-runtime] {dest} is up to date (OBS {VERSION})", file=sys.stderr)
        return
    old = read_manifest(dest)
    zip_path = fetch()
    entries: dict[str, str] = {}
    with zipfile.ZipFile(zip_path) as z:
        names = [n for n in z.namelist() if wanted(n)]
        missing = [b for b in BIN if f"bin/64bit/{b}" not in names] + [m for m in MODULES if f"obs-plugins/64bit/{m}.dll" not in names]
        if missing:
            sys.exit(f"[obs-runtime] not in {ZIP_NAME}: {', '.join(missing)}")
        files = {n: z.read(n) for n in names}
        files["licenses/gplv2.txt"] = z.read(LICENSE_SRC)
    files[NOTICE] = notice().replace("\n", "\r\n").encode()
    for name, data in sorted(files.items()):
        target = dest / name
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(data)
        entries[name] = hashlib.sha256(data).hexdigest()
    for name in old.keys() - entries.keys():
        (dest / name).unlink(missing_ok=True)
    lines = [stamp()] + [f"{digest}  {name}" for name, digest in sorted(entries.items())]
    manifest_path.write_text("\n".join(lines) + "\n")
    size = sum(len(d) for d in files.values())
    print(f"[obs-runtime] staged {len(files)} files ({size / 1e6:.1f} MB) from OBS {VERSION} into {dest}", file=sys.stderr)


def verify(dest: Path, quiet: bool = False) -> int:
    """Number of missing or changed files (prints them unless quiet)."""
    entries = read_manifest(dest)
    if not entries:
        if not quiet:
            print(f"[obs-runtime] no manifest in {dest}", file=sys.stderr)
        return 1
    bad = 0
    for name, digest in entries.items():
        path = dest / name
        if not path.exists() or sha256_file(path) != digest:
            bad += 1
            if not quiet:
                print(f"[obs-runtime] {'missing' if not path.exists() else 'changed'}: {name}", file=sys.stderr)
    if not quiet:
        print(f"[obs-runtime] {len(entries) - bad} of {len(entries)} files match", file=sys.stderr)
    return bad


def main(argv: list[str]) -> int:
    if len(argv) == 2 and argv[1] == "fetch":
        print(fetch())
        return 0
    if len(argv) == 3 and argv[1] == "stage":
        stage(Path(argv[2]))
        return 0
    if len(argv) == 3 and argv[1] == "verify":
        return 1 if verify(Path(argv[2])) else 0
    print(__doc__, file=sys.stderr)
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv))
