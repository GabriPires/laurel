#!/usr/bin/env python3
"""Gather the corresponding source of the third-party software in a Laurel release.

Laurel ships GPL binaries it did not build: OBS Studio's libobs, plugins and helpers
(tools/obs-runtime.py), the FFmpeg libraries and other libraries that OBS's obs-deps
builds into them, and its own ffmpeg.exe (tools/ffmpeg-build.py). Whoever distributes
them must offer their source, so each release carries Laurel-<version>-sources.zip,
made here from pinned sources (decision of 2026-10-24: mirror everything):

  - OBS Studio's source archive of the release, and FFmpeg's release archive, checked
    against the SHA-256 pins of those tools;
  - obs-deps at its tag (the build scripts, patches and licenses), and each library of
    obs-deps that reaches the shipped files, at the commit or download its scripts pin.
    Left out: libraries only for OBS plugins Laurel does not ship (luajit, python, swig,
    pcre, bison: scripting; asio, websocketpp, qrcodegencpp: websocket; libdatachannel:
    WebRTC; freetype: text; rnnoise, speexdsp: filters; vlc; libajantv2), build or test
    tools (gas-preprocessor for arm64, cmocka), and libpng and zstd, which leave no trace
    in the shipped files.

Git sources are fetched at their commit and archived with `git archive` (no checkout,
nothing from the repository runs); downloads are checked against obs-deps' SHA-256.
Laurel's own source is the release's source archive on GitHub.

Usage:
  python3 -I tools/gpl-sources.py fetch                      fill the cache
  python3 -I tools/gpl-sources.py bundle <version> <out-dir> write Laurel-<version>-sources.zip
"""

import hashlib
import importlib.util
import os
import shutil
import subprocess
import sys
import tempfile
import urllib.request
import zipfile
from pathlib import Path

OBS_DEPS = ("obs-deps", "2025-08-23", "https://github.com/obsproject/obs-deps.git", "21e25b2b508598ce8239de9ecd68400e45559399")

# (name, version, origin, commit or SHA-256, what uses it). Pins from obs-deps 2025-08-23
# (deps.ffmpeg/*.ps1, deps.windows/*.ps1 and their checksums).
# Parts of a repository, for sources obs-deps checks out only in part.
SPARSE = {"amf": ["amf/public/include", "LICENSE.txt"]}
GIT = [
    ("FFmpeg", "7.1.1", "https://github.com/FFmpeg/FFmpeg.git", "db69d06eeeab4f46da15030a80d539efb4503ca8", "avcodec, avdevice, avfilter, avformat, avutil, swresample, swscale (with obs-deps' patches)"),
    ("x264", "r3106", "https://github.com/mirror/x264.git", "eaa68fad9e5d201d42fde51665f2d137ae96baf0", "libx264-164.dll"),
    ("zlib", "1.3.1", "https://github.com/madler/zlib.git", "51b7f2abdade71cd9bb0e7a373ef2610ec6f9daf", "zlib.dll, FFmpeg"),
    ("opus", "1.5.2", "https://github.com/xiph/opus.git", "ddbe48383984d56acd9e1ab6a090c54ca6b735a6", "FFmpeg (libopus)"),
    ("libogg", "1.3.5", "https://github.com/xiph/ogg.git", "fa80aae9d50096160f2b56ada35527d7aee3f746", "FFmpeg (libvorbis, libtheora)"),
    ("libvorbis", "1.3.7", "https://gitlab.xiph.org/xiph/vorbis.git", "2eac96b03ff67953354cb0a649c08aa3a23267ef", "FFmpeg (libvorbis)"),
    ("svt-av1", "2.3.0", "https://gitlab.com/AOMediaCodec/SVT-AV1.git", "6e69def4ec283fe0b71195671245c3b768bebdef", "FFmpeg (libsvtav1)"),
    ("aom", "3.12.0", "https://aomedia.googlesource.com/aom.git", "3b624af45b86646a20b11a9ff803aeae588cdee6", "FFmpeg (libaom)"),
    ("mbedtls", "3.6.4", "https://github.com/Mbed-TLS/mbedtls.git", "c765c831e5c2a0971410692f92f7a81d6ec65ec2", "srt.dll, librist.dll"),
    ("librist", "0.2.7", "https://code.videolan.org/rist/librist.git", "809390b3b75a259a704079d0fb4d8f1b5f7fa956", "librist.dll"),
    ("nv-codec-headers", "12.2.72.0", "https://github.com/FFmpeg/nv-codec-headers.git", "c69278340ab1d5559c7d7bf0edf615dc33ddbba7", "FFmpeg (NVENC headers)"),
    ("amf", "1.4.36", "https://github.com/GPUOpen-LibrariesAndSDKs/AMF.git", "16f7d73e0b45c473e903e46981ed0b91efc4c091", "FFmpeg and obs-ffmpeg (AMF headers)"),
    ("curl", "8.12.1", "https://github.com/curl/curl.git", "57495c64871d18905a0941db9196ef90bafe9a29", "libcurl.dll"),
    ("jansson", "2.14.1", "https://github.com/akheron/jansson.git", "96d160df90016066d04d493d1d69639474ba4f20", "obs.dll, win-capture.dll"),
    ("detours", "4.0.1", "https://github.com/microsoft/detours.git", "e4bfd6b03e50de46b47abfbd1e46b384f0c5f833", "graphics-hook32.dll, graphics-hook64.dll"),
    ("simde", "0.8.2", "https://github.com/simd-everywhere/simde.git", "71fd833d9666141edcd1d3c109a80e228303d8d7", "libobs (headers)"),
    ("uthash", "2.3.0", "https://github.com/troydhanson/uthash.git", "e493aa90a2833b4655927598f169c31cfcdf7861", "libobs (headers)"),
    ("nlohmann-json", "3.11.3", "https://github.com/nlohmann/json.git", "9cca280a4d0ccf0c08f47a99aa71d1b0e52f8d03", "OBS plugins (headers)"),
    ("vpl", "2.14.0", "https://github.com/intel/libvpl.git", "025d43d086a3e663184cb49febe86152bf05409f", "obs-qsv11.dll, obs-qsv-test.exe"),
    ("wil", "1.0.240803.1", "https://github.com/microsoft/wil.git", "f0c6a81c0c9a4b23b6801f40554b8bec425a83b4", "OBS on Windows (headers)"),
]
FILES = [
    ("libvpx", "1.14.1", "https://github.com/webmproject/libvpx/archive/refs/tags/v1.14.1.zip", "f40e4c7e7b4025d18be17a8225f8be2f26b349715399cd36c2de573fe976dfd7", "FFmpeg (libvpx)", "libvpx-1.14.1.zip"),
    ("libtheora", "1.1.1", "https://ftp.osuosl.org/pub/xiph/releases/theora/libtheora-1.1.1.zip", "f644fef154f7a80e7258c8baec5c510f594d720835855cddce322b924934ba36", "FFmpeg (libtheora)", "libtheora-1.1.1.zip"),
    ("lame", "3.100", "https://downloads.sourceforge.net/project/lame/lame/3.100/lame-3.100.tar.gz", "ddfe36cab873794038ae2c1210557ad34857a4b6bdc515785d1da9e175b1da1e", "FFmpeg (libmp3lame)", "lame-3.100.tar.gz"),
    ("srt", "1.5.2", "https://github.com/Haivision/srt/archive/refs/tags/v1.5.2.zip", "2686f75533bd53ec917dab6129186b124b4d105eadaa6a84e83719ac3b5ae6bd", "srt.dll", "srt-1.5.2.zip"),
    ("vulkansdk", "1.3.216.0", "https://cdn-fastly.obsproject.com/downloads/VulkanSDK-1.3.216.0-Installer-Components.7z", "e17989a846de1fbfa738f9192f9ebd099379a79a2a8afe0f3971956540b79659", "win-capture and its hook (Vulkan headers)", "VulkanSDK-1.3.216.0-Installer-Components.7z"),
]


def log(msg: str) -> None:
    print(f"[gpl-sources] {msg}", file=sys.stderr)


def cache_dir() -> Path:
    base = os.environ.get("XDG_CACHE_HOME") or str(Path.home() / ".cache")
    return Path(base) / "laurel" / "gpl-sources" / "archives"


def tool(name: str):
    """Another tool of tools/ as a module (its pins and download helpers)."""
    spec = importlib.util.spec_from_file_location(name.replace("-", "_"), Path(__file__).with_name(f"{name}.py"))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def sha256_file(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as f:
        for block in iter(lambda: f.read(1 << 20), b""):
            h.update(block)
    return h.hexdigest()


def download(url: str, sha256: str, dest: Path, cached: Path | None = None) -> Path:
    """`dest`, from `cached` (another tool's download) or from `url`, checked."""
    if not dest.exists() and cached is not None and cached.exists():
        shutil.copyfile(cached, dest)
    for attempt in (1, 2):
        if dest.exists():
            break
        log(f"downloading {url}")
        try:
            with tempfile.NamedTemporaryFile(dir=dest.parent, delete=False) as tmp:
                with urllib.request.urlopen(url, timeout=60) as r:
                    for block in iter(lambda: r.read(1 << 20), b""):
                        tmp.write(block)
            os.replace(tmp.name, dest)
        except OSError as e:
            Path(tmp.name).unlink(missing_ok=True)
            if attempt == 2:
                sys.exit(f"[gpl-sources] {url}: {e}")
    actual = sha256_file(dest)
    if actual != sha256:
        sys.exit(f"[gpl-sources] {dest.name}: SHA-256 {actual} does not match the pinned {sha256}")
    return dest


def git_archive(name: str, version: str, url: str, commit: str) -> Path:
    """<name>-<version>-<commit[:12]>.tar.gz of the tree at `commit` (or of its SPARSE paths)."""
    stem = f"{name}-{version}-{commit[:12]}"
    dest = cache_dir() / f"{stem}.tar.gz"
    if dest.exists():
        return dest
    log(f"archiving {name} {version} at {commit[:12]} from {url}")
    with tempfile.TemporaryDirectory(dir=cache_dir()) as tmp:
        git = lambda *args, check=True: subprocess.run(["git", "-C", tmp, *args], check=check, capture_output=True)
        subprocess.run(["git", "init", "-q", tmp], check=True)
        # Servers that refuse a fetch by commit get a full fetch instead.
        if git("fetch", "-q", "--depth", "1", url, commit, check=False).returncode != 0:
            git("fetch", "-q", url, "+refs/heads/*:refs/remotes/origin/*", "+refs/tags/*:refs/tags/*")
        if git("rev-parse", "--verify", f"{commit}^{{commit}}").stdout.decode().strip() != commit:
            sys.exit(f"[gpl-sources] {name}: commit {commit} not found at {url}")
        git("archive", "--format=tar.gz", f"--prefix={stem}/", "-o", str(dest.with_suffix(".tmp")), commit, *SPARSE.get(name, []))
        os.replace(dest.with_suffix(".tmp"), dest)
    return dest


def entries():
    """(archive path, name, version, used by, origin, pin) for every source."""
    rt, fb = tool("obs-runtime"), tool("ffmpeg-build")
    obs_url = f"https://github.com/obsproject/obs-studio/releases/download/{rt.VERSION}/{rt.SOURCES_NAME}"
    rows = [
        (download(obs_url, rt.SOURCES_SHA256, cache_dir() / rt.SOURCES_NAME), "OBS Studio", rt.VERSION, "libobs, its plugins and helpers in engine\\", obs_url, f"SHA-256 {rt.SOURCES_SHA256}"),
        (download(fb.TAR_URL, fb.TAR_SHA256, cache_dir() / fb.TAR_NAME, fb.cache_dir() / fb.TAR_NAME), "FFmpeg (release)", fb.VERSION, "engine\\bin\\64bit\\ffmpeg.exe, built by tools/ffmpeg-build.py", fb.TAR_URL, f"SHA-256 {fb.TAR_SHA256}"),
    ]
    name, version, url, commit = OBS_DEPS
    rows.append((git_archive(name, version, url, commit), name, version, "builds the libraries below for OBS (scripts, patches, licenses)", url, f"commit {commit}"))
    for name, version, url, commit, used in GIT:
        part = f" (only {', '.join(SPARSE[name])}, the part obs-deps checks out)" if name in SPARSE else ""
        rows.append((git_archive(name, version, url, commit), name, version, used, url, f"commit {commit}{part}"))
    for name, version, url, sha, used, file in FILES:
        rows.append((download(url, sha, cache_dir() / file), name, version, used, url, f"SHA-256 {sha}"))
    return rows


def manifest(version: str, rows) -> str:
    lines = [
        f"Laurel {version}: source code of the third-party software it carries",
        "",
        "Laurel is free software under the GNU General Public License, version 2 or later.",
        "Its own source code is the release's source archive on GitHub (the tag of this",
        "release in the public repository). This archive holds the source of the software",
        "Laurel ships but does not build itself, as it was used for this release:",
        "",
        "- OBS Studio's libobs, plugins and helpers (engine\\ folder), unmodified from the",
        "  official OBS Studio build; OBS Studio is GPL-2.0-or-later.",
        "- The libraries OBS's obs-deps builds into them, among them FFmpeg with x264, which",
        "  obs-deps configures with --enable-gpl and --enable-version3 (GPL-3.0-or-later).",
        "- Laurel's ffmpeg.exe, FFmpeg's own program from the FFmpeg release below.",
        "",
        "Each archive keeps its own license files. For each item: version, what uses it,",
        "where it came from, its pin, and the archive here with its SHA-256.",
        "",
    ]
    for path, name, ver, used, origin, pin in rows:
        lines += [f"{name} {ver}", f"  used by: {used}", f"  origin:  {origin}", f"  pin:     {pin}", f"  archive: sources/{path.name}", f"           SHA-256 {sha256_file(path)}", ""]
    return "\r\n".join(lines)


def bundle(version: str, out: Path) -> Path:
    cache_dir().mkdir(parents=True, exist_ok=True)
    rows = entries()
    out.mkdir(parents=True, exist_ok=True)
    target = out / f"Laurel-{version}-sources.zip"
    tmp = target.with_suffix(".tmp")
    stamp = (2026, 1, 1, 0, 0, 0)
    with zipfile.ZipFile(tmp, "w", zipfile.ZIP_STORED) as z:
        z.writestr(zipfile.ZipInfo("SOURCES.txt", stamp), manifest(version, rows))
        for path, *_ in rows:
            info = zipfile.ZipInfo(f"sources/{path.name}", stamp)
            with path.open("rb") as src, z.open(info, "w", force_zip64=True) as dst:
                shutil.copyfileobj(src, dst, 1 << 20)
    os.replace(tmp, target)
    log(f"{target}: {len(rows)} sources, {target.stat().st_size} bytes, SHA-256 {sha256_file(target)}")
    return target


def main(argv: list[str]) -> int:
    if len(argv) == 2 and argv[1] == "fetch":
        cache_dir().mkdir(parents=True, exist_ok=True)
        rows = entries()
        log(f"{len(rows)} sources in {cache_dir()} ({sum(p.stat().st_size for p, *_ in rows)} bytes)")
        return 0
    if len(argv) == 4 and argv[1] == "bundle":
        print(bundle(argv[2], Path(argv[3])))
        return 0
    print(__doc__, file=sys.stderr)
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv))
