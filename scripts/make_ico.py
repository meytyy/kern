"""Packs PNG renditions of the KERN icon into a Windows .ico (PNG-compressed entries).

    python3 scripts/make_ico.py out.ico 16.png 24.png 32.png 48.png 64.png 128.png 256.png
"""
import struct
import sys


def size_of(png: bytes) -> tuple[int, int]:
    # IHDR follows the 8-byte signature and the chunk header.
    return struct.unpack(">II", png[16:24])


def main(out: str, files: list[str]) -> None:
    images = [open(f, "rb").read() for f in files]
    header = struct.pack("<HHH", 0, 1, len(images))
    offset = 6 + 16 * len(images)
    entries = b""
    for png in images:
        w, h = size_of(png)
        entries += struct.pack(
            "<BBBBHHII", w % 256, h % 256, 0, 0, 1, 32, len(png), offset
        )
        offset += len(png)
    with open(out, "wb") as f:
        f.write(header + entries + b"".join(images))


if __name__ == "__main__":
    main(sys.argv[1], sys.argv[2:])
