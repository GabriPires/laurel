"""Summarize PresentMon CSV captures: frames, average FPS and 1% / 0.1% lows.

Usage: python3 -I tools/fpssummary.py <label>=<file.csv> [<label>=<file.csv> ...]
Reads the PresentMon 2.x FrameTime column (ms), or MsBetweenPresents for --v1_metrics.
The "1% low" is the FPS at the 99th percentile frame time.
"""
import csv
import sys


def frame_times(path):
    with open(path, newline='', encoding='utf-8', errors='replace') as f:
        rows = list(csv.DictReader(f))
    if not rows:
        return []
    column = 'FrameTime' if 'FrameTime' in rows[0] else 'MsBetweenPresents'
    out = []
    for r in rows:
        try:
            v = float(r[column])
        except (KeyError, ValueError):
            continue
        if v > 0:
            out.append(v)
    return out


def percentile(sorted_values, pct):
    if not sorted_values:
        return float('nan')
    k = (len(sorted_values) - 1) * pct / 100
    lo, hi = int(k), min(int(k) + 1, len(sorted_values) - 1)
    return sorted_values[lo] + (sorted_values[hi] - sorted_values[lo]) * (k - lo)


def main():
    results = []
    for arg in sys.argv[1:]:
        label, path = arg.split('=', 1)
        ft = frame_times(path)
        if not ft:
            print(f'{label}: no frames in {path}')
            continue
        s = sorted(ft)
        avg_fps = 1000 / (sum(ft) / len(ft))
        low1 = 1000 / percentile(s, 99)
        low01 = 1000 / percentile(s, 99.9)
        results.append((label, avg_fps, low1))
        print(f'{label}: frames={len(ft)} avg_fps={avg_fps:.1f} low1_fps={low1:.1f} low0.1_fps={low01:.1f} '
              f'avg_frametime_ms={sum(ft) / len(ft):.2f} p99_frametime_ms={percentile(s, 99):.2f}')
    if len(results) >= 2:
        base = results[0]
        for label, avg, low1 in results[1:]:
            print(f'{label} vs {base[0]}: avg_fps {100 * (avg - base[1]) / base[1]:+.1f}%, '
                  f'low1_fps {100 * (low1 - base[2]) / base[2]:+.1f}%')


if __name__ == '__main__':
    main()
