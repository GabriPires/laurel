# Third-party notices

Laurel is licensed under the GNU General Public License, version 2 or (at your option) any later version (see `LICENSE`). This file lists the third-party software and assets that are in this repository, and the ones Laurel uses at build or run time without keeping them here.

## In this repository

| Component | Where | License |
|---|---|---|
| Cinzel, Figtree and JetBrains Mono fonts (Latin subsets from Google Fonts) | `app/ui/fonts/` | SIL Open Font License 1.1 (`app/ui/fonts/OFL-*.txt`) |
| Declarations of the libobs C API, written by hand from the OBS Studio headers | `engine/src/obs/ffi.rs` | OBS Studio is GPL-2.0-or-later |

Laurel's own icons (`app/icons/`) are drawn by `tools/make-icons.mjs`.

## Downloaded at build time, not kept here

- **OBS Studio 32.1.2** (libobs and the plugins `win-capture`, `win-wasapi`, `obs-ffmpeg`, `obs-x264`, `obs-nvenc`, `obs-qsv11`, `obs-filters`), GPL-2.0-or-later. `tools/obs-runtime.py` takes them from the official release on GitHub, checked against a pinned SHA-256, and keeps the GPL text and a source notice in the runtime's `licenses/` folder. The runtime also carries the libraries OBS ships with those plugins (for example FFmpeg's libraries), under their own licenses. Source: https://github.com/obsproject/obs-studio/tree/32.1.2.
- **RNNoise** and **SpeexDSP**, linked by OBS's obs-deps into the `obs-filters` plugin for the microphone's noise suppression, BSD-3-Clause; their license texts are under "RNNoise and SpeexDSP licenses" below.
- **FFmpeg 7.1.1** (`ffmpeg.exe`, the command-line program only) for trims, thumbnails and the timeline. `tools/ffmpeg-build.py` builds it, unmodified, from the official release (https://ffmpeg.org/releases/ffmpeg-7.1.1.tar.xz, checked against a pinned SHA-256) and links it to the FFmpeg 7.1.1 libraries of the OBS runtime, which OBS's obs-deps builds with `--enable-gpl --enable-version3`; this FFmpeg build is therefore GPL-3.0-or-later. The runtime's `licenses/` folder keeps the GPL 3 text and a notice with the source and the build options.
- **Rust crates**, among them Tauri 2 and rusqlite with a bundled SQLite (public domain). Their licenses are permissive (MIT, Apache-2.0, BSD, Zlib, Unicode-3.0, MPL-2.0 and similar); `Cargo.lock` pins every crate and version.

Each release carries `Laurel-<version>-sources.zip`, made by `tools/gpl-sources.py`: the source of OBS Studio, of FFmpeg, of OBS's obs-deps build scripts and of each library they build into the shipped files, at the versions, commits and SHA-256 sums those projects pin. Its `SOURCES.txt` says where each one came from and what uses it.

## In the installer (`tools/installer.sh`), downloaded at build time

- **Microsoft Visual C++ Redistributable (x64) 14.51.36247** (`vc_redist.x64.exe`), checked against a pinned SHA-256 and signed by Microsoft. The installer runs it only when the runtime is missing; it is distributed under Microsoft's license terms for the Visual C++ redistributable files and is not part of Laurel.
- **Microsoft Edge WebView2 bootstrapper**, which installs the WebView2 Runtime only when it is missing. Microsoft's license terms.
- **NSIS** (the installer's runtime, zlib/libpng license) and Tauri's NSIS plugin `nsis_tauri_utils` (MIT or Apache-2.0), added by Tauri's bundler.

## Used at run time, not distributed with Laurel

- **Microsoft Edge WebView2 Runtime**, part of Windows, renders the user interface.
- **Steam API library** (`steam_api64.dll`, Valve), already on the user's PC inside installed Steam games. Laurel's Steam helper (`laurel-steam.exe`, MIT license, see `steam/LICENSE`) loads it at run time to read the player's achievements for the running game; Laurel does not ship it or any Steamworks SDK file. Valve's public Web API gives the achievements' global percentages.
- **Microsoft Visual C++ Runtime**, needed by the engine, OBS and FFmpeg; the installer adds it when it is missing.

## Development tools, not distributed

PresentMon (FPS measurements), a full GPL build of FFmpeg from BtbN/FFmpeg-Builds (`ffprobe.exe` checks clips in tests), Microsoft Edge (to draw the icons and check the UI), and the Rust, Python and Node toolchains.

## RNNoise and SpeexDSP licenses

The `obs-filters` plugin of the OBS runtime carries RNNoise and SpeexDSP, built by OBS's obs-deps. Their licenses, as obs-deps keeps them:

### RNNoise

```
Copyright (c) 2017, Mozilla
Copyright (c) 2007-2017, Jean-Marc Valin
Copyright (c) 2005-2017, Xiph.Org Foundation
Copyright (c) 2003-2004, Mark Borgerding

Redistribution and use in source and binary forms, with or without
modification, are permitted provided that the following conditions
are met:

- Redistributions of source code must retain the above copyright
notice, this list of conditions and the following disclaimer.

- Redistributions in binary form must reproduce the above copyright
notice, this list of conditions and the following disclaimer in the
documentation and/or other materials provided with the distribution.

- Neither the name of the Xiph.Org Foundation nor the names of its
contributors may be used to endorse or promote products derived from
this software without specific prior written permission.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS
``AS IS'' AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT
LIMITED TO, THE IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR
A PARTICULAR PURPOSE ARE DISCLAIMED.  IN NO EVENT SHALL THE FOUNDATION
OR CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL,
SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT
LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES; LOSS OF USE,
DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON ANY
THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT
(INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE
OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.
```

### SpeexDSP

```
Copyright 2002-2008 	Xiph.org Foundation
Copyright 2002-2008 	Jean-Marc Valin
Copyright 2005-2007	Analog Devices Inc.
Copyright 2005-2008	Commonwealth Scientific and Industrial Research 
                        Organisation (CSIRO)
Copyright 1993, 2002, 2006 David Rowe
Copyright 2003 		EpicGames
Copyright 1992-1994	Jutta Degener, Carsten Bormann

Redistribution and use in source and binary forms, with or without
modification, are permitted provided that the following conditions
are met:

- Redistributions of source code must retain the above copyright
notice, this list of conditions and the following disclaimer.

- Redistributions in binary form must reproduce the above copyright
notice, this list of conditions and the following disclaimer in the
documentation and/or other materials provided with the distribution.

- Neither the name of the Xiph.org Foundation nor the names of its
contributors may be used to endorse or promote products derived from
this software without specific prior written permission.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS
``AS IS'' AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT
LIMITED TO, THE IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR
A PARTICULAR PURPOSE ARE DISCLAIMED.  IN NO EVENT SHALL THE FOUNDATION OR
CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL,
EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT LIMITED TO,
PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES; LOSS OF USE, DATA, OR
PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON ANY THEORY OF
LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT (INCLUDING
NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE OF THIS
SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.
```

## Trademarks

Laurel is an independent project. It is not affiliated with, endorsed by or sponsored by Medal B.V.; "Medal" and "Medal.tv" are names and marks of their owner, used here only to describe the functional reference of this project. OBS Studio is a project of the OBS Project; its name appears here only to credit the libraries Laurel uses. Game names that appear in the documentation belong to their owners.
