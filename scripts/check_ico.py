"""Checks a Windows .ico: sizes, transparent corners, even margins.

    python3 scripts/check_ico.py assets/windows/kern.ico [sheet.png [other.ico]]

With sheet.png it also draws every size, enlarged without smoothing, on a light
and a dark background (and `other.ico` below, to compare). Standard library only.
"""
import struct
import sys
import zlib

WANT = [16, 24, 32, 48, 64, 128, 256]


def png_rgba(data: bytes):
    assert data[:8] == b"\x89PNG\r\n\x1a\n"
    i, idat = 8, b""
    while i < len(data):
        (ln,) = struct.unpack(">I", data[i : i + 4])
        kind, chunk = data[i + 4 : i + 8], data[i + 8 : i + 8 + ln]
        if kind == b"IHDR":
            w, h, depth, ctype = struct.unpack(">IIBB", chunk[:10])
            assert depth == 8 and ctype == 6, "expected 8-bit RGBA"
        elif kind == b"IDAT":
            idat += chunk
        i += 12 + ln
    raw, stride, out, prev, p = zlib.decompress(idat), w * 4, [], bytearray(w * 4), 0
    for _ in range(h):
        f, line = raw[p], bytearray(raw[p + 1 : p + 1 + stride])
        p += 1 + stride
        for x in range(stride):
            a = line[x - 4] if x >= 4 else 0
            b, c = prev[x], (prev[x - 4] if x >= 4 else 0)
            if f == 1:
                line[x] = (line[x] + a) & 255
            elif f == 2:
                line[x] = (line[x] + b) & 255
            elif f == 3:
                line[x] = (line[x] + (a + b) // 2) & 255
            elif f == 4:
                pa, pb, pc = abs(b - c), abs(a - c), abs(a + b - 2 * c)
                line[x] = (line[x] + (a if pa <= pb and pa <= pc else b if pb <= pc else c)) & 255
        out.append(bytes(line))
        prev = line
    return w, h, out


def bmp_rgba(data: bytes, size: int):
    hdr, w, h2, planes, bpp = struct.unpack("<IiiHH", data[:16])
    assert hdr == 40 and w == size and h2 == 2 * size and bpp == 32, (hdr, w, h2, bpp)
    rows = []
    for y in range(size - 1, -1, -1):
        start = 40 + y * size * 4
        row = bytearray()
        for x in range(size):
            b, g, r, a = data[start + x * 4 : start + x * 4 + 4]
            row += bytes([r, g, b, a])
        rows.append(bytes(row))
    return size, size, rows


def entries(path: str):
    d = open(path, "rb").read()
    reserved, kind, count = struct.unpack("<HHH", d[:6])
    assert reserved == 0 and kind == 1
    for i in range(count):
        w, h, _, _, _, bpp, size, off = struct.unpack("<BBBBHHII", d[6 + 16 * i : 22 + 16 * i])
        w, h, blob = w or 256, h or 256, d[off : off + size]
        fmt = "PNG" if blob[:4] == b"\x89PNG" else "BMP"
        _, _, rows = png_rgba(blob) if fmt == "PNG" else bmp_rgba(blob, w)
        yield w, fmt, bpp, rows


def check(path: str) -> bool:
    ok, seen = True, []
    for size, fmt, bpp, rows in entries(path):
        seen.append(size)
        alpha = lambda x, y: rows[y][x * 4 + 3]
        corners = [alpha(0, 0), alpha(size - 1, 0), alpha(0, size - 1), alpha(size - 1, size - 1)]
        opaque = [(x, y) for y in range(size) for x in range(size) if alpha(x, y) > 128]
        xs, ys = [p[0] for p in opaque], [p[1] for p in opaque]
        margins = (min(xs), size - 1 - max(xs), min(ys), size - 1 - max(ys))
        even = max(margins) - min(margins) <= 1
        line = f"{size:>3}px {fmt} {bpp}bit corners alpha={corners} margins l/r/t/b={margins}"
        good = all(a == 0 for a in corners) and even and bpp == 32
        print(("ok   " if good else "FAIL ") + line)
        ok &= good
    if sorted(seen) != WANT:
        print(f"FAIL sizes {sorted(seen)}, want {WANT}")
        ok = False
    return ok


def sheet(out: str, paths: list[str]) -> None:
    scale_for = {16: 6, 24: 4, 32: 3, 48: 2, 64: 2, 128: 1, 256: 1}
    panels = []
    for path in paths:
        for bg in [(243, 243, 243), (32, 32, 32)]:
            items = [(s, rows, scale_for[s]) for s, _, _, rows in entries(path)]
            panels.append((bg, sorted(items)))
    width = sum(s * k + 16 for s, _, k in panels[0][1]) + 16
    height = sum(max(s * k for s, _, k in items) + 16 for _, items in panels) + 16
    canvas = [bytearray(width * 3) for _ in range(height)]
    y0 = 16
    for bg, items in panels:
        row_h = max(s * k for s, _, k in items)
        for y in range(y0 - 8, y0 + row_h + 8):
            for x in range(width):
                canvas[y][x * 3 : x * 3 + 3] = bytes(bg)
        x0 = 16
        for s, rows, k in items:
            for y in range(s * k):
                for x in range(s * k):
                    r, g, b, a = rows[y // k][(x // k) * 4 : (x // k) * 4 + 4]
                    px = canvas[y0 + y]
                    o = (x0 + x) * 3
                    for c, v in enumerate((r, g, b)):
                        px[o + c] = (v * a + px[o + c] * (255 - a)) // 255
            x0 += s * k + 16
        y0 += row_h + 16
    raw = b"".join(b"\x00" + bytes(r) for r in canvas)
    chunk = lambda t, d: struct.pack(">I", len(d)) + t + d + struct.pack(">I", zlib.crc32(t + d))
    png = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0))
    png += chunk(b"IDAT", zlib.compress(raw, 9)) + chunk(b"IEND", b"")
    open(out, "wb").write(png)


if __name__ == "__main__":
    passed = check(sys.argv[1])
    if len(sys.argv) > 2:
        sheet(sys.argv[2], [sys.argv[1]] + sys.argv[3:])
    sys.exit(0 if passed else 1)
