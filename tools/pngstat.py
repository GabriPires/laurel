"""Report whether PNG screenshots are black: size, mean brightness and lit pixels.

Usage: python3 -I tools/pngstat.py [--crop X,Y,W,H] [--near RRGGBB[:TOL]] <file.png> ...
Supports the 8-bit RGB/RGBA non-interlaced PNGs written by laurel-engine.
A pixel counts as lit when any colour channel is above 16.
--crop limits the statistics to a region; --near also counts the pixels whose
channels are all within TOL (default 40) of a colour, e.g. to tell whether a
notice of a known colour appears in a capture.
"""
import struct
import sys
import zlib


def decode(path):
    data = open(path, 'rb').read()
    if data[:8] != b'\x89PNG\r\n\x1a\n':
        raise ValueError('not a PNG')
    pos, idat, header = 8, b'', None
    while pos < len(data):
        length, kind = struct.unpack('>I4s', data[pos:pos + 8])
        body = data[pos + 8:pos + 8 + length]
        if kind == b'IHDR':
            header = struct.unpack('>IIBBBBB', body)
        elif kind == b'IDAT':
            idat += body
        pos += 12 + length
    width, height, depth, color, _, _, interlace = header
    if depth != 8 or color not in (2, 6) or interlace:
        raise ValueError(f'unsupported PNG (depth {depth}, color type {color}, interlace {interlace})')
    bpp = 4 if color == 6 else 3
    raw = zlib.decompress(idat)
    stride = width * bpp
    rows, prev = [], bytearray(stride)
    for y in range(height):
        f = raw[y * (stride + 1)]
        line = bytearray(raw[y * (stride + 1) + 1:(y + 1) * (stride + 1)])
        for i in range(stride):
            a = line[i - bpp] if i >= bpp else 0
            b = prev[i]
            c = prev[i - bpp] if i >= bpp else 0
            if f == 1:
                line[i] = (line[i] + a) & 255
            elif f == 2:
                line[i] = (line[i] + b) & 255
            elif f == 3:
                line[i] = (line[i] + (a + b) // 2) & 255
            elif f == 4:
                p = a + b - c
                pa, pb, pc = abs(p - a), abs(p - b), abs(p - c)
                line[i] = (line[i] + (a if pa <= pb and pa <= pc else b if pb <= pc else c)) & 255
        rows.append(line)
        prev = line
    return width, height, bpp, rows


def main():
    args, crop, near = sys.argv[1:], None, None
    while args and args[0].startswith('--'):
        flag, value = args[0], args[1]
        args = args[2:]
        if flag == '--crop':
            crop = tuple(int(v) for v in value.split(','))
        elif flag == '--near':
            hexcode, _, tol = value.partition(':')
            near = (tuple(int(hexcode[i:i + 2], 16) for i in (0, 2, 4)), int(tol or 40))
    for path in args:
        width, height, bpp, rows = decode(path)
        x0, y0, w, h = crop or (0, 0, width, height)
        step = 1 if crop else 4  # whole images: every 4th row and column is enough
        total = lit = brightness = close = 0
        for line in rows[y0:y0 + h:step]:
            for x in range(x0 * bpp, (x0 + w) * bpp, step * bpp):
                r, g, b = line[x], line[x + 1], line[x + 2]
                total += 1
                brightness += (r + g + b) / 3
                lit += max(r, g, b) > 16
                if near and all(abs(c - t) <= near[1] for c, t in zip((r, g, b), near[0])):
                    close += 1
        region = f' crop={x0},{y0},{w},{h}' if crop else ''
        extra = f' near_{value_hex(near[0])}={close}' if near else ''
        print(f'{path}: {width}x{height}{region} mean={brightness / total:.1f} lit={100 * lit / total:.1f}%{extra}'
              f'{" BLACK" if lit == 0 else ""}')


def value_hex(rgb):
    return ''.join(f'{c:02X}' for c in rgb)


if __name__ == '__main__':
    main()
