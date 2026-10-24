#!/usr/bin/env bash
# Sample resource usage of a Windows process and of the NVIDIA GPU from WSL.
#
# Usage: tools/perfsample.sh <process-name-without-.exe> <seconds> [interval]
# CPU time is read as integer milliseconds to avoid locale decimal separators.
# Prints CSV: t_s,proc_cpu_pct,proc_ws_mb,gpu_util_pct,enc_util_pct,vram_used_mb
# proc_cpu_pct is relative to one logical core (100 = one core fully busy).
set -euo pipefail

proc=${1:?usage: tools/perfsample.sh <process> <seconds> [interval]}
duration=${2:?usage: tools/perfsample.sh <process> <seconds> [interval]}
interval=${3:-2}
nvsmi=$(command -v nvidia-smi || echo /usr/lib/wsl/lib/nvidia-smi)

echo "t_s,proc_cpu_pct,proc_ws_mb,gpu_util_pct,enc_util_pct,vram_used_mb"
start=$(date +%s.%N)
prev_cpu=''
prev_t=''
while :; do
  now=$(date +%s.%N)
  t=$(awk -v a="$now" -v b="$start" 'BEGIN{printf "%.1f", a-b}')
  awk -v t="$t" -v d="$duration" 'BEGIN{exit !(t>d)}' && break
  read -r cpu ws < <(powershell.exe -NoProfile -Command \
    "\$p = Get-Process -Name '$proc' -ErrorAction SilentlyContinue | Select-Object -First 1; if (\$p) { '{0} {1}' -f [int64]\$p.TotalProcessorTime.TotalMilliseconds, \$p.WorkingSet64 } else { '- -' }" \
    | tr -d '\r') || true
  gpu=$("$nvsmi" --query-gpu=utilization.gpu,utilization.encoder,memory.used --format=csv,noheader,nounits 2>/dev/null | head -1 | tr -d ' ' || echo ',,')
  cpu_pct=''
  if [ "${cpu:--}" != '-' ] && [ -n "$prev_cpu" ]; then
    cpu_pct=$(awk -v c="$cpu" -v pc="$prev_cpu" -v n="$now" -v pn="$prev_t" 'BEGIN{printf "%.1f", (c-pc)/1000/(n-pn)*100}')
  fi
  ws_mb=''
  [ "${ws:--}" != '-' ] && ws_mb=$(awk -v w="$ws" 'BEGIN{printf "%.0f", w/1048576}')
  echo "$t,$cpu_pct,$ws_mb,$gpu"
  [ "${cpu:--}" != '-' ] && { prev_cpu=$cpu; prev_t=$now; }
  sleep "$interval"
done
