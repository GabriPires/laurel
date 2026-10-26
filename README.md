# Laurel

**A free, open-source game clip recorder for Windows. No ads, no paywall, no account: every feature is included.**

Laurel keeps the last seconds of the game you are playing and saves them as a clip when you press a hotkey. It finds your games on its own, records their sound apart from your microphone, and keeps everything on your PC, in a library where you trim, keep and share your clips.

> **Status: release candidate.** Laurel 0.1.0-rc.4 is a pre-release for a test with friends: an installer for Windows 10 (2004 or later) and 11, 64-bit, that updates itself, in English or Brazilian Portuguese. It is not signed yet, so Windows SmartScreen warns before it runs. The test guide, in Portuguese, is in [`docs/teste-dos-amigos.md`](docs/teste-dos-amigos.md).

*Resumo em português mais abaixo.*

## What it does today

- **Clips with one key.** A replay buffer holds the last 30 seconds at 60 fps; F8 saves a clip and Shift+F8 a screenshot. The hotkeys work in exclusive fullscreen, the game still gets the key, and mouse side buttons can be used too.
- **Finds your games.** Laurel reads the games installed through Steam, Epic Games, Riot and Xbox, plus a curated list and the games you add yourself. It starts capturing when a game opens and stops when it closes. There is no capture source to pick.
- **Game audio and your voice, apart.** The game's own sound (not Discord or music) and the microphone go on separate tracks, with a mix on the first one.
- **Captures that hold up.** Game capture through OBS Studio's hook, a window capture fallback, an offer to restart as administrator for games that need it, and an optional screen recording when no game is open (off by default).
- **A library made for clips.** Clips and screenshots grouped by day, filtered by game, searched and sorted; a muted preview when the pointer rests on a clip; Keep to protect a clip from the size limit; several items selected at once; Delete to Recently deleted, with Undo, where items wait 1 to 30 days before they go for good.
- **A viewer with a trim.** Laurel's own player, a timeline of frames with two handles, frame-exact cuts (a fast copy or an exact re-encode), copy the file to paste it in Discord, or show it in its folder.
- **Clips of your Steam achievements.** While a Steam game is recorded, Laurel can save a clip by itself when you unlock an achievement: any, rare (under 10% of players) or ultra rare (under 5%), with games left out on request. The library marks these clips with a trophy and filters them.
- **Stays out of the way.** Notices over the game, sounds, start with Windows, a tray icon that shows whether Laurel is recording, a storage limit and a pause when the disk is almost full.

## How it is built

| Part | What it is |
|---|---|
| `engine/` | `laurel-engine`: the recording engine, written in Rust on top of OBS Studio's **libobs**. It runs headless and talks JSON-RPC over stdio. |
| `app/` | `laurel-app`: the desktop app in Rust and **Tauri 2** (WebView2 UI). Tray, game detection, hotkeys (Raw Input), overlay, library (SQLite) and trims (FFmpeg). |
| `steam/` | `laurel-steam`: a small helper under the **MIT license**, apart from the GPL code. While a Steam game runs, it loads the Steam API library already on the PC (the game's own `steam_api64.dll`, or another installed game's) and reports new achievement unlocks to the app. Laurel ships nothing from Valve. |
| `tools/` | Build, packaging and test scripts. |
| `poc/` | Throwaway feasibility spikes. |

Laurel never uses an installed OBS Studio: `tools/obs-runtime.py` stages its own minimal runtime from the official OBS Studio 32.1.2 release, checked against a pinned SHA-256, with only the plugins the engine loads.

## Building

Laurel is cross-compiled for Windows (MSVC) from WSL. You need Rust with the `x86_64-pc-windows-msvc` target, `cargo-xwin`, `clang`/`lld`/`llvm`, `make` and `gcc` (for FFmpeg's `configure`), Python 3 and Node.

```bash
rustup target add x86_64-pc-windows-msvc
cargo install --locked cargo-xwin

# Build the engine, stage the OBS runtime and run it once
tools/winrun.sh engine --obs -- <work-dir>

# Build and start the app (it finds the engine staged above)
tools/winrun.sh app

# The self-contained Laurel folder and a zip of it
tools/package.sh

# The Windows installer (NSIS), and its test on a clean Windows in Windows Sandbox
cargo install tauri-cli --version "^2" --locked
tools/installer.sh
tools/sandboxtest.sh
```

`tools/winrun.sh` builds, copies the result to `%USERPROFILE%\laurel-dev\` and runs it on Windows. Trims and thumbnails use a small `ffmpeg.exe` (about 0.3 MB) that `tools/ffmpeg-build.py` builds from the official FFmpeg 7.1.1 release, checked against a pinned SHA-256, and links to the FFmpeg libraries already in the OBS runtime; `tools/winrun.sh engine --obs` and `tools/package.sh` put it next to the engine (`LAUREL_FFMPEG` points the app to another one). `tools/winrun.sh app -- --selftest` runs the app's scripted self-test (it types F13, F14 and F16 into the focused window, so run it when you are not playing).

`tools/installer.sh` builds `Laurel_<version>_x64-setup.exe` with Tauri's NSIS bundler. It installs Laurel for the current user in `%LOCALAPPDATA%\Laurel`, without administrator rights, turns on start with Windows on a fresh install (the app confirms it on its first run), and installs Microsoft's Visual C++ Runtime only when it is missing (that step asks for administrator rights). The uninstaller removes Laurel, its start entry and, when no OBS Studio is installed, the game capture hook it shares with OBS; clips are never removed, and the app's data only when you tick the box. Without `makensis` on the PATH, the script unpacks Ubuntu's `nsis` packages into its cache, without root.

Laurel updates itself through the Tauri updater: a little after it starts (unless turned off in Settings, About), it asks the public repository's update channel whether a new version exists, and installs it only when you click Install and restart. Packages must carry the project's signature, with the version signed in. `tools/sign-release.sh` signs an installer with the project's private key (it asks for the key's password) and writes the channel's `latest.json`.

## How the project works

- **Documentation first.** Every behavior is written as a spec before it is built, and every decision and test is recorded, in the project's knowledge base (in Brazilian Portuguese). The knowledge base is private for now; this repository holds the product. Code, commits and identifiers are in English.
- **Medal as a functional reference, clean-room.** Laurel takes the Medal desktop app as its reference for what a clip recorder should do, and writes its own specs from that. No Medal code, assets or data are in this repository or in Laurel. Laurel does not modify or unlock the official client and does not use Medal's API or cloud.
- **Built with AI agents.** The code and the documentation are written with AI coding agents (Claude Code), guided and tested by the project owner.
- **Local-first.** No account, no server, no third-party telemetry. Sharing happens by exporting files.

## Roadmap

1. **Now (Phase 4, MVP):** stage 6, with encoders for NVIDIA, AMD, Intel and the CPU, the default audio track (game only; game and microphone; all PC sound and microphone), a small FFmpeg, and an installer with a clean uninstall. Then a test with friends.
2. **Phase 5, parity:** the features of the reference app that fit Laurel, starting with a vertical (TikTok-style) export.
3. **Phase 6:** first release and community.

## License

GPL-2.0-or-later, the same license as libobs (see `LICENSE`), except `steam/` (the Steam helper), which is MIT (see `steam/LICENSE`). Third-party software and assets are listed in [`THIRD-PARTY-NOTICES.md`](THIRD-PARTY-NOTICES.md).

Laurel is an independent project, not affiliated with, endorsed by or sponsored by Medal B.V.

---

## Em português

O **Laurel** é um gravador de clipes para jogos no Windows, gratuito e de código aberto: sem anúncios, sem paywall e sem conta, com todos os recursos liberados. Ele guarda os últimos segundos do jogo e salva um clipe quando você aperta um atalho (F8). Ele encontra os seus jogos sozinho e grava o som do jogo separado do microfone. Tudo fica no seu PC, numa biblioteca com prévia, corte por quadro, seleção de vários itens e uma área de excluídos recentemente, com Desfazer. O Laurel fala português e inglês.

O projeto está em desenvolvimento e em teste com amigos: a 0.1.0-rc.4 é uma pré-release com instalador para Windows, que se atualiza sozinha. A documentação, com specs, decisões e testes, é escrita em português e por enquanto fica numa base privada; este repositório tem o produto. O Medal serve de referência funcional, com sala limpa: nenhum código, recurso ou dado do Medal entra no repositório nem no Laurel. O Laurel não tem nenhuma ligação com a Medal B.V.

Licença: GPL-2.0-or-later.
