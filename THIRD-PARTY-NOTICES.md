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
- **Rust crates**, among them Tauri 2 and rusqlite with a bundled SQLite (public domain). Their licenses are permissive (MIT, Apache-2.0, BSD, Zlib, Unicode-3.0, MPL-2.0 and similar); `Cargo.lock` pins every crate and version.

## Used at run time, not distributed with Laurel today

- **FFmpeg** (`ffmpeg.exe`, `ffprobe.exe`) for trims, thumbnails and the timeline. The development setup uses a GPL build from BtbN/FFmpeg-Builds. How Laurel ships FFmpeg is decided before the first release.
- **Microsoft Edge WebView2 Runtime**, part of Windows, renders the user interface.

## Development tools, not distributed

PresentMon (FPS measurements), Microsoft Edge (to draw the icons and check the UI), and the Rust, Python and Node toolchains.

## Trademarks

Laurel is an independent project. It is not affiliated with, endorsed by or sponsored by Medal B.V.; "Medal" and "Medal.tv" are names and marks of their owner, used here only to describe the functional reference of this project. OBS Studio is a project of the OBS Project; its name appears here only to credit the libraries Laurel uses. Game names that appear in the documentation belong to their owners.
