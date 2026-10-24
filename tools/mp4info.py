"""Print duration and tracks of an MP4 file by reading its boxes (no ffprobe needed).

Usage: python3 -I tools/mp4info.py <file.mp4> [...]
Output per file: size, movie duration, and per track: handler (vide/soun), codec
fourcc, duration, sample count, and for video the tkhd width/height.
"""
import struct
import sys

CONTAINERS = {b'moov', b'trak', b'mdia', b'minf', b'stbl', b'edts', b'udta'}


def boxes(data, start, end):
    pos = start
    while pos + 8 <= end:
        size, kind = struct.unpack_from('>I4s', data, pos)
        header = 8
        if size == 1:
            size = struct.unpack_from('>Q', data, pos + 8)[0]
            header = 16
        elif size == 0:
            size = end - pos
        if size < header:
            break
        yield kind, pos + header, pos + size
        pos += size


def find(data, start, end, path):
    for kind, s, e in boxes(data, start, end):
        if kind == path[0]:
            if len(path) == 1:
                return s, e
            return find(data, s, e, path[1:])
    return None


def full_box_times(data, s):
    version = data[s]
    if version == 1:
        timescale, duration = struct.unpack_from('>IQ', data, s + 4 + 16)
    else:
        timescale, duration = struct.unpack_from('>II', data, s + 4 + 8)
    return timescale, duration


def info(path):
    with open(path, 'rb') as f:
        data = f.read()
    print(f'{path}: {len(data)} bytes')
    moov = find(data, 0, len(data), [b'moov'])
    if not moov:
        print('  no moov box (file not finalized?)')
        return
    mvhd = find(data, *moov, [b'mvhd'])
    ts, dur = full_box_times(data, mvhd[0])
    print(f'  duration {dur / ts:.3f}s')
    n = 0
    for kind, s, e in boxes(data, *moov):
        if kind != b'trak':
            continue
        n += 1
        hdlr = find(data, s, e, [b'mdia', b'hdlr'])
        handler = data[hdlr[0] + 8:hdlr[0] + 12].decode('latin-1')
        mdhd = find(data, s, e, [b'mdia', b'mdhd'])
        tts, tdur = full_box_times(data, mdhd[0])
        stsd = find(data, s, e, [b'mdia', b'minf', b'stbl', b'stsd'])
        codec = data[stsd[0] + 12:stsd[0] + 16].decode('latin-1')
        stsz = find(data, s, e, [b'mdia', b'minf', b'stbl', b'stsz'])
        samples = struct.unpack_from('>I', data, stsz[0] + 8)[0] if stsz else -1
        extra = ''
        if handler == 'vide':
            tkhd = find(data, s, e, [b'tkhd'])
            w, h = struct.unpack_from('>II', data, tkhd[1] - tkhd[0] - 8 + tkhd[0])
            extra = f' {w >> 16}x{h >> 16}'
        print(f'  track {n}: {handler} {codec} {tdur / tts:.3f}s samples={samples}{extra}')


if __name__ == '__main__':
    for p in sys.argv[1:]:
        info(p)
