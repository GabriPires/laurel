# Third-party notices

Laurel is licensed under the GNU General Public License, version 2 or (at your option) any later version (see `LICENSE`). This file lists the third-party software and assets that are in this repository, and the ones Laurel uses at build or run time without keeping them here.

## In this repository

| Component | Where | License |
|---|---|---|
| Cinzel, Figtree and JetBrains Mono fonts (Latin subsets from Google Fonts) | `app/ui/fonts/` | SIL Open Font License 1.1 (`app/ui/fonts/OFL-*.txt`) |
| Declarations of the libobs C API, written by hand from the OBS Studio headers | `engine/src/obs/ffi.rs` | OBS Studio is GPL-2.0-or-later |

Laurel's own icons (`app/icons/`) are drawn by `tools/make-icons.mjs`.

## Downloaded at build time, not kept here

- **OBS Studio 32.1.2** (libobs and the plugins `win-capture`, `win-wasapi`, `obs-ffmpeg`, `obs-x264`, `obs-nvenc`, `obs-qsv11`), GPL-2.0-or-later. `tools/obs-runtime.py` takes them from the official release on GitHub, checked against a pinned SHA-256, and keeps the GPL text and a source notice in the runtime's `licenses/` folder. The runtime also carries the libraries OBS ships with those plugins (for example FFmpeg's libraries), under their own licenses. Source: https://github.com/obsproject/obs-studio/tree/32.1.2.
- **FFmpeg 7.1.1** (`ffmpeg.exe`, the command-line program only) for trims, thumbnails and the timeline. `tools/ffmpeg-build.py` builds it, unmodified, from the official release (https://ffmpeg.org/releases/ffmpeg-7.1.1.tar.xz, checked against a pinned SHA-256) and links it to the FFmpeg 7.1.1 libraries of the OBS runtime, which OBS's obs-deps builds with `--enable-gpl --enable-version3`; this FFmpeg build is therefore GPL-3.0-or-later. The runtime's `licenses/` folder keeps the GPL 3 text and a notice with the source and the build options.
- **Rust crates**, among them Tauri 2 and rusqlite with a bundled SQLite (public domain). Their licenses are permissive (MIT, Apache-2.0, BSD, Zlib, Unicode-3.0, MPL-2.0 and similar); `Cargo.lock` pins every crate and version.

Each release carries `Laurel-<version>-sources.zip`, made by `tools/gpl-sources.py`: the source of OBS Studio, of FFmpeg, of OBS's obs-deps build scripts and of each library they build into the shipped files, at the versions, commits and SHA-256 sums those projects pin. Its `SOURCES.txt` says where each one came from and what uses it.

## In the installer (`tools/installer.sh`), downloaded at build time

- **Microsoft Visual C++ Redistributable (x64) 14.51.36247** (`vc_redist.x64.exe`), checked against a pinned SHA-256 and signed by Microsoft. The installer runs it only when the runtime is missing; it is distributed under Microsoft's license terms for the Visual C++ redistributable files and is not part of Laurel.
- **Microsoft Edge WebView2 bootstrapper**, which installs the WebView2 Runtime only when it is missing. Microsoft's license terms.
- **NSIS** (the installer's runtime, zlib/libpng license) and Tauri's NSIS plugin `nsis_tauri_utils` (MIT or Apache-2.0), added by Tauri's bundler.

## Used at run time, not distributed with Laurel

- **Microsoft Edge WebView2 Runtime**, part of Windows, renders the user interface.
- **Microsoft Visual C++ Runtime**, needed by the engine, OBS and FFmpeg; the installer adds it when it is missing.

## Development tools, not distributed

PresentMon (FPS measurements), a full GPL build of FFmpeg from BtbN/FFmpeg-Builds (`ffprobe.exe` checks clips in tests), Microsoft Edge (to draw the icons and check the UI), and the Rust, Python and Node toolchains.

## Trademarks

Laurel is an independent project. It is not affiliated with, endorsed by or sponsored by Medal B.V.; "Medal" and "Medal.tv" are names and marks of their owner, used here only to describe the functional reference of this project. OBS Studio is a project of the OBS Project; its name appears here only to credit the libraries Laurel uses. Game names that appear in the documentation belong to their owners.
