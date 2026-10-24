#!/usr/bin/env bash
# Build a Rust crate for Windows (MSVC) from WSL, copy the result to a local
# Windows folder and run it there through WSL interop (decision D10, option B).
#
# Usage: tools/winrun.sh <crate-dir> [--obs] [--timeout SECONDS] [-- args...]
#   --obs       stage Laurel's own OBS runtime (tools/obs-runtime.py) in the output
#               folder and put the executable in its bin\64bit, next to obs.dll
# Output goes to C:\Users\$WINUSER\laurel-dev\<crate-name>\ (with --obs, the
# executable goes to its bin\64bit\); args are passed as-is.
set -euo pipefail

# The Windows user, unless WINUSER says otherwise.
WINUSER=${WINUSER:-$(cd /mnt/c && cmd.exe /c 'echo %USERNAME%' 2>/dev/null | tr -d '\r')}
ROOT=$(cd "$(dirname "$0")/.." && pwd)
TARGET=x86_64-pc-windows-msvc

crate=${1:?usage: tools/winrun.sh <crate-dir> [--obs] [--timeout S] [-- args...]}
shift
use_obs=0
timeout_s=120
while [ $# -gt 0 ]; do
  case "$1" in
    --obs) use_obs=1; shift ;;
    --timeout) timeout_s=$2; shift 2 ;;
    --) shift; break ;;
    *) break ;;
  esac
done

export PATH=$HOME/.cargo/bin:$PATH
read -r name target_dir < <(cd "$crate" && cargo metadata --no-deps --format-version 1 | python3 -I -c '
import json, os, sys
m = json.load(sys.stdin)
here = os.path.realpath(os.getcwd())
pkg = next(p for p in m["packages"] if os.path.dirname(os.path.realpath(p["manifest_path"])) == here)
print(pkg["name"], m["target_directory"])')
(cd "$crate" && cargo xwin build --release --target "$TARGET" -p "$name")

dest_wsl=/mnt/c/Users/$WINUSER/laurel-dev/$name
dest_win="C:\\Users\\$WINUSER\\laurel-dev\\$name"
if [ "$use_obs" = 1 ]; then
  python3 -I "$ROOT/tools/obs-runtime.py" stage "$dest_wsl"
  dest_wsl=$dest_wsl/bin/64bit
  dest_win=$dest_win\\bin\\64bit
fi
mkdir -p "$dest_wsl"
cp "$target_dir/$TARGET/release/$name.exe" "$dest_wsl/"
pdb="$target_dir/$TARGET/release/${name//-/_}.pdb"
[ -f "$pdb" ] && cp "$pdb" "$dest_wsl/"

{
  printf '@echo off\r\n'
  printf '"%%~dp0%s.exe"' "$name"
  for a in "$@"; do printf ' "%s"' "$a"; done
  printf '\r\nexit /b %%ERRORLEVEL%%\r\n'
} > "$dest_wsl/run.cmd"

cd "$dest_wsl"
set +e
timeout "$timeout_s" cmd.exe /c run.cmd 2>&1 | tr -d '\r'
rc=${PIPESTATUS[0]}
echo "[winrun] $name exited with $rc ($dest_win)"
exit "$rc"
