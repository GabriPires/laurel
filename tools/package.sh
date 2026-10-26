#!/usr/bin/env bash
# Build Laurel and assemble its self-contained folder, which needs no OBS Studio
# install (Phase 4, stage 1):
#
#   <out>\Laurel\laurel-app.exe
#   <out>\Laurel\laurel-steam.exe and LICENSE-laurel-steam.txt (the MIT Steam helper)
#   <out>\Laurel\LICENSE
#   <out>\Laurel\engine\            OBS runtime staged by tools/obs-runtime.py
#   <out>\Laurel\engine\bin\64bit\laurel-engine.exe
#   <out>\Laurel\engine\bin\64bit\ffmpeg.exe  from tools/ffmpeg-build.py (trims, thumbnails)
#
# and a zip of it next to the folder, to copy to another PC.
#
# Usage: tools/package.sh [--out WSL-DIR] [--no-zip]
#   default out: /mnt/c/Users/$WINUSER/laurel-dev (C:\Users\$WINUSER\laurel-dev)
set -euo pipefail

# The Windows user, unless WINUSER says otherwise.
WINUSER=${WINUSER:-$(cd /mnt/c && cmd.exe /c 'echo %USERNAME%' 2>/dev/null | tr -d '\r')}
ROOT=$(cd "$(dirname "$0")/.." && pwd)
TARGET=x86_64-pc-windows-msvc
out=/mnt/c/Users/$WINUSER/laurel-dev
zip=1
while [ $# -gt 0 ]; do
  case "$1" in
    --out) out=$2; shift 2 ;;
    --no-zip) zip=0; shift ;;
    *) echo "usage: tools/package.sh [--out WSL-DIR] [--no-zip]" >&2; exit 2 ;;
  esac
done

export PATH=$HOME/.cargo/bin:$PATH
cd "$ROOT"
cargo xwin build --release --target "$TARGET" -p laurel-engine -p laurel-app -p laurel-steam
release=$ROOT/target/$TARGET/release
version=$(cargo metadata --no-deps --format-version 1 | python3 -I -c '
import json, sys
print(next(p["version"] for p in json.load(sys.stdin)["packages"] if p["name"] == "laurel-app"))')

dest=$out/Laurel
mkdir -p "$dest/engine/bin/64bit"
python3 -I "$ROOT/tools/obs-runtime.py" stage "$dest/engine"
python3 -I "$ROOT/tools/ffmpeg-build.py" stage "$dest/engine"
cp "$release/laurel-engine.exe" "$dest/engine/bin/64bit/"
cp "$release/laurel-app.exe" "$release/laurel-steam.exe" "$ROOT/LICENSE" "$ROOT/THIRD-PARTY-NOTICES.md" "$dest/"
cp "$ROOT/steam/LICENSE" "$dest/LICENSE-laurel-steam.txt"
echo "[package] Laurel $version in $dest"

if [ "$zip" = 1 ]; then
  archive=$out/Laurel-$version-win-x64.zip
  python3 -I - "$out" "$archive" <<'EOF'
import os, sys, zipfile
out, archive = sys.argv[1], sys.argv[2]
with zipfile.ZipFile(archive + ".tmp", "w", zipfile.ZIP_DEFLATED) as z:
    for folder, _, files in os.walk(os.path.join(out, "Laurel")):
        for name in sorted(files):
            path = os.path.join(folder, name)
            z.write(path, os.path.relpath(path, out))
os.replace(archive + ".tmp", archive)
EOF
  echo "[package] $archive ($(du -h "$archive" | cut -f1))"
fi
