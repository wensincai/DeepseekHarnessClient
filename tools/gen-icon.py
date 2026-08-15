#!/usr/bin/env python3
"""Generate a 1024x1024 app icon (dark gradient + rounded blue chip + three bars).

Pure stdlib (zlib/struct) — no PIL needed. Run:
    python tools/gen-icon.py tools/icon.png
Then regenerate the Tauri icon set with:  npm run icons
"""

import os
import struct
import sys
import zlib

W = H = 1024
OUT = sys.argv[1] if len(sys.argv) > 1 else os.path.join(
    os.path.dirname(__file__), "icon.png")


def chunk(tag: bytes, data: bytes) -> bytes:
    return (
        struct.pack(">I", len(data))
        + tag
        + data
        + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)
    )


def in_rounded_rect(x, y, cx, cy, half, radius):
    """True if (x,y) is inside a rect centered at (cx,cy) with rounded corners."""
    dx = abs(x - cx)
    dy = abs(y - cy)
    if dx <= half and dy <= half:
        return True
    # corner zone: outside the axis-aligned rect but within the corner radius
    if dx > half and dy > half:
        ox = dx - half
        oy = dy - half
        return ox * ox + oy * oy <= radius * radius
    return False


def bars_zone(x, y, cx, cy):
    """Three vertical bars as a hint of "DSH" text."""
    for off in (-110, 0, 110):
        if abs(x - (cx + off)) <= 16 and abs(y - cy) <= 140:
            return True
    return False


def px(x, y):
    t = (x + y) / (W + H)
    r = int(22 + 24 * t)
    g = int(28 + 34 * t)
    b = int(50 + 62 * t)

    if in_rounded_rect(x, y, W / 2, H / 2, 270, 130):
        vt = y / H
        r = int(36 + 8 * vt)
        g = int(96 + 18 * vt)
        b = int(235 + 18 * vt)
        if bars_zone(x, y, W / 2, H / 2):
            r, g, b = 226, 236, 255

    return r, g, b


def write_png(path):
    rows = bytearray()
    for y in range(H):
        rows.append(0)  # filter: none
        for x in range(W):
            r, g, b = px(x, y)
            rows += bytes((r, g, b, 255))
    raw = bytes(rows)
    png = b"\x89PNG\r\n\x1a\n"
    png += chunk(b"IHDR", struct.pack(">IIBBBBB", W, H, 8, 6, 0, 0, 0))
    png += chunk(b"IDAT", zlib.compress(raw, 9))
    png += chunk(b"IEND", b"")
    with open(path, "wb") as f:
        f.write(png)


if __name__ == "__main__":
    write_png(OUT)
    print(f"wrote {OUT} ({W}x{H})")
