#!/usr/bin/env bash
# Measure Laurel's impact on a running game: FPS without and with laurel-engine
# recording, engine resource use, hook result and the saved clip.
#
# Usage: tools/gametest.sh <game-exe> [--label NAME] [--seconds N] [--warmup W]
#                          [--base WxH] [--bitrate KBPS] [--encoder ID] [--fullscreen]
#                          [--no-presentmon [--lead L]]
#                          [--start CAPTURE] [--script SCRIPT] [--no-output]
# The game must already be running and in gameplay. Steps:
#   1. PresentMon for N seconds without Laurel (baseline)
#   2. laurel-engine starts game capture of the game's window (or any fullscreen app
#      with --fullscreen) and records; after W seconds of warm-up, PresentMon runs
#      for N seconds while resource usage is sampled
#   3. the engine saves a clip and exits; the script prints FPS deltas, engine
#      stats, hook log lines and the clip tracks
# PresentMon needs admin rights or membership in "Performance Log Users".
# With --no-presentmon the player reads FPS from an overlay (e.g. Steam): the script
# waits L seconds (baseline, default 20), beeps once when recording starts and twice
# when it ends.
# Stutter isolation: --start sets the initial capture (e.g. monitor:1) and --script
# replaces the engine script; "{game}" in it expands to the game capture target, e.g.
#   --start monitor:1 --script 'wait 30; beep 2; capture {game}; wait 30; stats; save; beep 3; quit'
# The run length is the sum of the script's waits; with --script the closing beeps
# come from the script. --no-output captures without encoders or replay buffer.
set -euo pipefail

ROOT=$(cd "$(dirname "$0")/.." && pwd)
# The Windows user, unless WINUSER says otherwise.
WINUSER=${WINUSER:-$(cd /mnt/c && cmd.exe /c 'echo %USERNAME%' 2>/dev/null | tr -d '\r')}
DEV_WSL=/mnt/c/Users/$WINUSER/laurel-dev
DEV_WIN="C:\\Users\\$WINUSER\\laurel-dev"
PM="$DEV_WSL/tools/PresentMon-2.6.0-x64.exe"

game=${1:?usage: tools/gametest.sh <game-exe> [options]}
shift
label=${game%.exe}; seconds=45; warmup=10; base=2560x1440; bitrate=20000; encoder=''; fullscreen=0; use_pm=1; lead=20
start=''; script=''; no_output=0
while [ $# -gt 0 ]; do
  case "$1" in
    --label) label=$2; shift 2 ;;
    --seconds) seconds=$2; shift 2 ;;
    --warmup) warmup=$2; shift 2 ;;
    --base) base=$2; shift 2 ;;
    --bitrate) bitrate=$2; shift 2 ;;
    --encoder) encoder=$2; shift 2 ;;
    --fullscreen) fullscreen=1; shift ;;
    --no-presentmon) use_pm=0; shift ;;
    --lead) lead=$2; shift 2 ;;
    --start) start=$2; shift 2 ;;
    --script) script=$2; shift 2 ;;
    --no-output) no_output=1; shift ;;
    *) echo "unknown option $1" >&2; exit 2 ;;
  esac
done

tasklist.exe /FI "IMAGENAME eq $game" 2>/dev/null | grep -i -F "${game:0:25}" >/dev/null || { echo "$game is not running" >&2; exit 1; }

stamp=$(date +%Y%m%d-%H%M%S)
out_wsl="$DEV_WSL/gametests/$label-$stamp"
out_win="$DEV_WIN\\gametests\\$label-$stamp"
mkdir -p "$out_wsl"
echo "[gametest] results in $out_win"

beep() { # <count>
  for _ in $(seq 1 "$1"); do powershell.exe -NoProfile -Command '[console]::Beep(988,350)' >/dev/null 2>&1 || true; sleep 0.15; done
}

presentmon() { # <csv-name> <seconds>
  (cd "$DEV_WSL/tools" && timeout $(( $2 + 30 )) ./PresentMon-2.6.0-x64.exe --process_name "$game" \
    --output_file "$out_win\\$1" --timed "$2" --terminate_after_timed --stop_existing_session --no_console_stats \
    >"$out_wsl/${1%.csv}.log" 2>&1) || true
}

if [ "$fullscreen" = 1 ]; then
  capture=game
else
  spec=''
  for _ in 1 2 3 4 5 6 7 8 9 10 11 12; do  # the window may appear after the process
    spec=$("$ROOT/tools/winrun.sh" "$ROOT/engine" --obs --timeout 90 -- "$DEV_WIN\\laurel-engine\\work" --list 2>/dev/null \
      | grep -F "[laurel] game window [$game]" | head -1 | sed 's/^.* = //') || true
    [ -n "$spec" ] && break
    sleep 10
  done
  [ -n "$spec" ] || { echo "no window found for $game; try --fullscreen" >&2; exit 1; }
  capture="game:$spec"
  echo "[gametest] game window: $spec"
fi

if [ "$use_pm" = 1 ]; then
  echo "[gametest] baseline: PresentMon ${seconds}s without Laurel"
  presentmon baseline.csv "$seconds"
else
  echo "[gametest] baseline window: ${lead}s without Laurel (read the FPS overlay)"
  sleep "$lead"
  beep 1
fi

if [ -n "$script" ]; then
  script=${script//\{game\}/$capture}
  run_s=$(grep -oE '(^|;) *wait +[0-9]+' <<<"$script" | grep -oE '[0-9]+' | paste -sd+ | bc)
  warmup=0; seconds=$run_s
  echo "[gametest] engine script (${run_s}s): $script"
else
  script="wait $(( warmup + seconds + 2 )); stats; save; stats; quit"
fi
engine_args=(--capture "${start:-$capture}" --base "$base" --bitrate "$bitrate" --buffer 30 --script "$script")
[ "$no_output" = 1 ] && engine_args+=(--no-output)
[ -n "$encoder" ] && engine_args+=(--encoder "$encoder")
echo "[gametest] starting laurel-engine (warm-up ${warmup}s)"
"$ROOT/tools/winrun.sh" "$ROOT/engine" --obs --timeout $(( warmup + seconds + 90 )) -- "$DEV_WIN\\laurel-engine\\work" "${engine_args[@]}" \
  >"$out_wsl/engine.log" 2>&1 &
engine_pid=$!
"$ROOT/tools/perfsample.sh" laurel-engine $(( warmup + seconds + 8 )) 2 >"$out_wsl/perf.csv" 2>&1 &
if [ "$use_pm" = 1 ]; then
  sleep "$warmup"
  echo "[gametest] with Laurel: PresentMon ${seconds}s"
  presentmon laurel.csv "$seconds"
fi
wait "$engine_pid" || true
[ "$use_pm" = 1 ] || [[ "$script" == *beep* ]] || beep 2
sleep 2

if [ "$use_pm" = 1 ]; then
  echo "== FPS"
  python3 -I "$ROOT/tools/fpssummary.py" "baseline=$out_wsl/baseline.csv" "laurel=$out_wsl/laurel.csv" || true
fi
echo "== engine"
grep -E '^\[laurel\] (recording|stats|clip|capture)|game-capture|Hooked|hook' "$out_wsl/engine.log" | cut -c1-240 || true
echo "== resources (engine process, GPU)"
python3 -I - "$out_wsl/perf.csv" <<'PY'
import csv, sys
rows = [r for r in csv.DictReader(open(sys.argv[1])) if r.get('proc_ws_mb')]
cpu = [float(r['proc_cpu_pct']) for r in rows if r['proc_cpu_pct']]
if rows:
    print(f"cpu_avg={sum(cpu) / max(len(cpu), 1):.1f}% of one core, ws_max={max(int(r['proc_ws_mb']) for r in rows)} MB, "
          f"gpu_util={min(int(r['gpu_util_pct']) for r in rows)}-{max(int(r['gpu_util_pct']) for r in rows)}%, "
          f"nvenc={min(int(r['enc_util_pct']) for r in rows)}-{max(int(r['enc_util_pct']) for r in rows)}%")
PY
clip=$(grep -oE 'clip saved in [0-9]+ ms: .*' "$out_wsl/engine.log" | sed 's/^.*: //' | tr -d '\r' | tail -1)
if [ -n "$clip" ]; then
  echo "== clip"
  python3 -I "$ROOT/tools/mp4info.py" "$(wslpath "$clip")"
fi
