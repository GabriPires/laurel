#!/usr/bin/env bash
# Build Laurel's Windows installer (Phase 4, stage 6): NSIS through Tauri's bundler,
# cross-compiled in WSL (decision D10). The installer puts Laurel in the user's
# %LOCALAPPDATA%\Laurel without administrator rights:
#
#   laurel-app.exe, LICENSE, THIRD-PARTY-NOTICES.md
#   engine\                  OBS runtime (tools/obs-runtime.py) with laurel-engine.exe
#                            and ffmpeg.exe (tools/ffmpeg-build.py) in engine\bin\64bit
#
# It also carries Microsoft's Visual C++ Runtime installer, run only when the runtime
# is missing, and WebView2's bootstrapper. Laurel's NSIS hooks are in
# app/installer/hooks.nsh and the bundle settings in app/tauri.installer.conf.json.
#
# Needs: cargo-xwin, the Tauri CLI (cargo install tauri-cli --version "^2" --locked)
# and makensis; without makensis on PATH the script unpacks Ubuntu's nsis packages
# into ~/.cache/laurel/nsis (apt-get download, no root).
#
# Usage: tools/installer.sh [--out WSL-DIR] [--config JSON-FILE]...
#   default out: /mnt/c/Users/$WINUSER/laurel-dev (C:\Users\$WINUSER\laurel-dev)
#   --config     more Tauri settings merged over the others (tests: another version
#                or update channel)
set -euo pipefail

# The Windows user, unless WINUSER says otherwise.
WINUSER=${WINUSER:-$(cd /mnt/c && cmd.exe /c 'echo %USERNAME%' 2>/dev/null | tr -d '\r')}
ROOT=$(cd "$(dirname "$0")/.." && pwd)
TARGET=x86_64-pc-windows-msvc
CACHE=${XDG_CACHE_HOME:-$HOME/.cache}/laurel
out=/mnt/c/Users/$WINUSER/laurel-dev
extra=()
while [ $# -gt 0 ]; do
  case "$1" in
    --out) out=$2; shift 2 ;;
    --config) extra+=(--config "$(realpath "$2")"); shift 2 ;;
    *) echo "usage: tools/installer.sh [--out WSL-DIR] [--config JSON-FILE]..." >&2; exit 2 ;;
  esac
done

# Microsoft Visual C++ v14 Redistributable (x64) 14.51.36247, Authenticode-signed by
# Microsoft Corporation (checked when pinned); aka.ms/vs/18/release/vc_redist.x64.exe.
VC_URL=https://download.visualstudio.microsoft.com/download/pr/ebdab8e5-1d7b-4d9f-a11b-cbb1720c3b12/843068991DAAA1F73AD9F6239BCE4D0F6A07A51F18C37EA2A867E9BECA71295C/VC_redist.x64.exe
VC_SHA256=843068991daaa1f73ad9f6239bce4d0f6a07a51f18c37ea2a867e9beca71295c

export PATH=$HOME/.cargo/bin:$PATH
command -v cargo-tauri >/dev/null || { echo "[installer] the Tauri CLI is missing: cargo install tauri-cli --version '^2' --locked" >&2; exit 1; }

# makensis: the system one, or Ubuntu's packages unpacked into the cache.
if ! command -v makensis >/dev/null; then
  nsis=$CACHE/nsis
  if [ ! -x "$nsis/bin/makensis" ]; then
    rm -rf "$nsis" && mkdir -p "$nsis/deb" "$nsis/bin"
    (cd "$nsis/deb" && apt-get download nsis nsis-common)
    for deb in "$nsis"/deb/*.deb; do dpkg -x "$deb" "$nsis/root"; done
    printf '#!/bin/sh\nNSISDIR="%s/root/usr/share/nsis" exec "%s/root/usr/bin/makensis" "$@"\n' "$nsis" "$nsis" > "$nsis/bin/makensis"
    chmod +x "$nsis/bin/makensis"
  fi
  export PATH=$nsis/bin:$PATH
fi
echo "[installer] makensis $(makensis -VERSION)"

cd "$ROOT"
stage=$ROOT/target/laurel-installer
mkdir -p "$stage/engine/bin/64bit"
cargo xwin build --release --target "$TARGET" -p laurel-engine
python3 -I tools/obs-runtime.py stage "$stage/engine"
python3 -I tools/ffmpeg-build.py stage "$stage/engine"
cp "target/$TARGET/release/laurel-engine.exe" "$stage/engine/bin/64bit/"

vc=$CACHE/vcredist/vc_redist.x64.exe
if [ ! -f "$vc" ] || [ "$(sha256sum "$vc" | cut -d' ' -f1)" != "$VC_SHA256" ]; then
  mkdir -p "$(dirname "$vc")"
  echo "[installer] downloading $VC_URL"
  curl -sSfL -o "$vc.tmp" "$VC_URL" && mv "$vc.tmp" "$vc"
fi
actual=$(sha256sum "$vc" | cut -d' ' -f1)
[ "$actual" = "$VC_SHA256" ] || { echo "[installer] $vc: SHA-256 $actual does not match the pinned $VC_SHA256" >&2; exit 1; }
cp "$vc" "$stage/vc_redist.x64.exe"

# The Tauri CLI on Linux looks up the tray library of Linux packages (appindicator)
# with pkg-config whenever the tray is used, even for a Windows build; a stand-in
# pkg-config answers that lookup during this step only.
fake=$CACHE/tauri-pkgconfig
mkdir -p "$fake"
printf '#!/bin/sh\n# Stand-in for the Tauri CLI tray library lookup (tools/installer.sh).\nfor a in "$@"; do [ "$a" = ayatana-appindicator3-0.1 ] && echo -L/usr/lib && exit 0; done\nexit 1\n' > "$fake/pkg-config"
chmod +x "$fake/pkg-config"
(cd app && PATH=$fake:$PATH TAURI_LINUX_AYATANA_APPINDICATOR=1 LAUREL_VC_REDIST=$stage/vc_redist.x64.exe cargo tauri build --runner cargo-xwin --target "$TARGET" --bundles nsis --config tauri.installer.conf.json "${extra[@]}")

setup=$(ls -t target/$TARGET/release/bundle/nsis/Laurel_*_x64-setup.exe | head -1)
mkdir -p "$out"
cp "$setup" "$out/"
echo "[installer] $out/$(basename "$setup") ($(stat -c %s "$setup") bytes, SHA-256 $(sha256sum "$setup" | cut -d' ' -f1))"
