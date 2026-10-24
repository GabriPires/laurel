#!/usr/bin/env bash
# Test Laurel's installer on a clean Windows: Windows Sandbox runs tools/sandbox-test.ps1
# on the newest installer from tools/installer.sh and writes its findings to
# %USERPROFILE%\laurel-dev\sandbox (result.txt, first-run.png, logs\), shared with
# the sandbox. Needs the Windows Sandbox feature (Windows Pro or Enterprise):
#   Enable-WindowsOptionalFeature -Online -FeatureName Containers-DisposableClientVM -All
# in an administrator PowerShell, then a restart.
#
# Usage: tools/sandboxtest.sh [--timeout SECONDS]
set -euo pipefail

# The Windows user, unless WINUSER says otherwise.
WINUSER=${WINUSER:-$(cd /mnt/c && cmd.exe /c 'echo %USERNAME%' 2>/dev/null | tr -d '\r')}
ROOT=$(cd "$(dirname "$0")/.." && pwd)
timeout_s=900
while [ $# -gt 0 ]; do
  case "$1" in
    --timeout) timeout_s=$2; shift 2 ;;
    *) echo "usage: tools/sandboxtest.sh [--timeout SECONDS]" >&2; exit 2 ;;
  esac
done

[ -e /mnt/c/Windows/System32/WindowsSandbox.exe ] || { echo "[sandbox] Windows Sandbox is not enabled (see the top of this script)" >&2; exit 1; }
dev=/mnt/c/Users/$WINUSER/laurel-dev
setup=$(ls -t "$dev"/Laurel_*_x64-setup.exe 2>/dev/null | head -1)
[ -n "$setup" ] || { echo "[sandbox] no installer in $dev: run tools/installer.sh" >&2; exit 1; }

box=$dev/sandbox
rm -rf "$box"
mkdir -p "$box"
cp "$setup" "$ROOT/tools/sandbox-test.ps1" "$box/"
cat > "$box/laurel.wsb" <<EOF
<Configuration>
  <MappedFolders>
    <MappedFolder>
      <HostFolder>C:\\Users\\$WINUSER\\laurel-dev\\sandbox</HostFolder>
      <SandboxFolder>C:\\laurel</SandboxFolder>
      <ReadOnly>false</ReadOnly>
    </MappedFolder>
  </MappedFolders>
  <LogonCommand>
    <Command>powershell.exe -NoProfile -ExecutionPolicy Bypass -File C:\\laurel\\sandbox-test.ps1</Command>
  </LogonCommand>
</Configuration>
EOF

echo "[sandbox] testing $(basename "$setup") in Windows Sandbox (close its window when this ends)"
(cd "$box" && cmd.exe /c start "" laurel.wsb)
for _ in $(seq "$timeout_s"); do
  if [ -f "$box/result.txt" ] && grep -q ' done' "$box/result.txt"; then break; fi
  sleep 1
done
[ -f "$box/result.txt" ] && cat "$box/result.txt" | tr -d '\r'
grep -q ' done' "$box/result.txt" 2>/dev/null || { echo "[sandbox] no result after ${timeout_s}s" >&2; exit 1; }
