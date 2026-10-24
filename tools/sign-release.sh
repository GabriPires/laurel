#!/usr/bin/env bash
# Sign Laurel's installer for the updater and write the update manifest (Phase 4,
# stage 6, item 5). Run it in your own terminal: it asks for the password of the
# project's private key, which never goes in the repository.
#
# It signs the newest installer from tools/installer.sh with its version (the app
# requires the signed version, plugins.updater.requireSignedVersion), and writes
# target/laurel-release/ with the installer, its .sig and latest.json. latest.json goes
# to the "updates" release of the public repository (the endpoint in tauri.conf.json);
# the installer and its .sig go to the release v<version>, where latest.json points.
#
# Usage: tools/sign-release.sh [--key FILE] [--notes TEXT]
#   default key: ~/.tauri/laurel.key (its public half, FILE.pub, must match tauri.conf.json)
set -euo pipefail

# The Windows user, unless WINUSER says otherwise.
WINUSER=${WINUSER:-$(cd /mnt/c && cmd.exe /c 'echo %USERNAME%' 2>/dev/null | tr -d '\r')}
ROOT=$(cd "$(dirname "$0")/.." && pwd)
REPO=https://github.com/GabriPires/laurel
key=$HOME/.tauri/laurel.key
notes=""
while [ $# -gt 0 ]; do
  case "$1" in
    --key) key=$2; shift 2 ;;
    --notes) notes=$2; shift 2 ;;
    *) echo "usage: tools/sign-release.sh [--key FILE] [--notes TEXT]" >&2; exit 2 ;;
  esac
done

export PATH=$HOME/.cargo/bin:$PATH
[ -f "$key" ] || { echo "[sign] no private key at $key: cargo tauri signer generate -w $key" >&2; exit 1; }
configured=$(python3 -I -c 'import json, sys; print(json.load(open(sys.argv[1]))["plugins"]["updater"]["pubkey"])' "$ROOT/app/tauri.conf.json")
[ "$(cat "$key.pub")" = "$configured" ] || { echo "[sign] $key.pub is not the public key in app/tauri.conf.json" >&2; exit 1; }

setup=$(ls -t /mnt/c/Users/$WINUSER/laurel-dev/Laurel_*_x64-setup.exe | head -1)
name=$(basename "$setup")
version=${name#Laurel_}
version=${version%_x64-setup.exe}
out=$ROOT/target/laurel-release
rm -rf "$out" && mkdir -p "$out"
cp "$setup" "$out/"
echo "[sign] signing $name for version $version"
cargo tauri signer sign -f "$key" --app-version "$version" "$out/$name"

python3 -I - "$out" "$name" "$version" "$REPO" "$notes" <<'EOF'
import datetime, json, pathlib, sys
out, name, version, repo, notes = pathlib.Path(sys.argv[1]), *sys.argv[2:]
manifest = {
    "version": version,
    "notes": notes,
    "pub_date": datetime.datetime.now(datetime.timezone.utc).replace(microsecond=0).isoformat().replace("+00:00", "Z"),
    "platforms": {
        "windows-x86_64": {
            "signature": (out / f"{name}.sig").read_text().strip(),
            "url": f"{repo}/releases/download/v{version}/{name}",
        }
    },
}
(out / "latest.json").write_text(json.dumps(manifest, indent=2) + "\n")
EOF
echo "[sign] $out: $name, $name.sig and latest.json (installer at $REPO/releases/download/v$version/$name)"
