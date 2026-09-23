#!/usr/bin/env python3
"""Decodes a 16-bit RGB PNG with only the standard library and counts the
distinct green levels in the bottom row. An 8-bit limited-range source has
at most 220 luma levels (16..235), so more than that on a row of grey
proves the snapshot kept more than 8 bits."""
import struct, sys, zlib

def read(path):
    b = open(path, "rb").read()
    assert b[:8] == b"\x89PNG\r\n\x1a\n"
    pos, idat = 8, b""
    while pos < len(b):
        n, t = struct.unpack(">I4s", b[pos:pos + 8])
        d = b[pos + 8:pos + 8 + n]
        if t == b"IHDR":
            w, h, depth, ctype = struct.unpack(">IIBB", d[:10])
        elif t == b"IDAT":
            idat += d
        pos += 12 + n
    assert depth == 16 and ctype == 2, (depth, ctype)
    raw, bpp, stride = zlib.decompress(idat), 6, w * 6
    rows, prev, i = [], bytearray(stride), 0
    for _ in range(h):
        f, line = raw[i], bytearray(raw[i + 1:i + 1 + stride]); i += 1 + stride
        for x in range(stride):
            a = line[x - bpp] if x >= bpp else 0
            up, c = prev[x], prev[x - bpp] if x >= bpp else 0
            if f == 1: line[x] = (line[x] + a) & 255
            elif f == 2: line[x] = (line[x] + up) & 255
            elif f == 3: line[x] = (line[x] + (a + up) // 2) & 255
            elif f == 4:
                p = a + up - c; pa, pb, pc = abs(p - a), abs(p - up), abs(p - c)
                line[x] = (line[x] + (a if pa <= pb and pa <= pc else up if pb <= pc else c)) & 255
        rows.append(line); prev = line
    return w, h, rows

for path in sys.argv[1:]:
    w, h, rows = read(path)
    row = rows[h - 1]  # bottom row: the ramp (and, in the harness pattern, a diagonal ramp)
    g = [struct.unpack(">H", row[x * 6 + 2:x * 6 + 4])[0] for x in range(w)]
    levels = sorted(set(g))
    print(f"{path}: {w}x{h} 16-bit RGB; bottom row has {len(levels)} distinct green levels")
