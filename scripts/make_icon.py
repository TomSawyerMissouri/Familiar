#!/usr/bin/env python3
"""
Draws the Familiar app icon.

The icon is described by a few shapes (signed distance functions) and rendered
at every size the macOS bundle needs, so each size is sharp rather than a
scaled-down copy. Only the Python standard library is used.

  python3 scripts/make_icon.py            # writes crates/familiar-mac/icons/*.png

The picture: a dark instrument panel with an amber reticle, and inside it a
person with a fainter copy behind them: a face seen before.
"""

import argparse
import math
import os
import struct
import zlib

# The standard macOS icon set: every size in points, at normal and at double
# ("@2x", Retina) pixel density. cargo-bundle packs these into the .icns file
# and tells the two densities apart by the "@2x" in the file name.
POINT_SIZES = [16, 32, 128, 256, 512]

PANEL_TOP = (30, 37, 44)
PANEL_BOTTOM = (11, 14, 17)
OUTLINE = (52, 62, 71)
AMBER = (232, 163, 61)
FIGURE = (224, 230, 234)

CENTER = 0.5
BODY_HALF = 0.402  # the icon body leaves the margin macOS icons have
BODY_RADIUS = 0.18
RING_RADIUS = 0.285
RING_WIDTH = 0.024


def rounded_square(x, y, half, radius):
    """Signed distance to a rounded square around the centre."""
    qx = abs(x - CENTER) - half + radius
    qy = abs(y - CENTER) - half + radius
    outside = math.hypot(max(qx, 0.0), max(qy, 0.0))
    return outside + min(max(qx, qy), 0.0) - radius


def ellipse(x, y, cx, cy, rx, ry):
    """Approximate signed distance to an ellipse, good enough for soft edges."""
    k = math.hypot((x - cx) / rx, (y - cy) / ry)
    return (k - 1.0) * min(rx, ry)


def person(x, y, dx, dy):
    """A head and shoulders, moved by (dx, dy)."""
    head = ellipse(x, y, CENTER + dx, 0.425 + dy, 0.088, 0.108)
    shoulders = ellipse(x, y, CENTER + dx, 0.800 + dy, 0.215, 0.205)
    return min(head, shoulders)


def reticle(x, y):
    """A ring with four ticks pointing outwards."""
    r = math.hypot(x - CENTER, y - CENTER)
    ring = abs(r - RING_RADIUS) - RING_WIDTH / 2
    ax, ay = abs(x - CENTER), abs(y - CENTER)
    along, across = (ax, ay) if ax > ay else (ay, ax)
    tick = max(across - RING_WIDTH / 2, RING_RADIUS - along, along - RING_RADIUS - 0.06)
    return min(ring, tick)


def render(size):
    """Returns the icon as rows of RGBA bytes."""
    pixel = 1.0 / size

    def coverage(distance):
        return min(max(0.5 - distance / pixel, 0.0), 1.0)

    rows = []
    for j in range(size):
        y = (j + 0.5) * pixel
        row = bytearray()
        for i in range(size):
            x = (i + 0.5) * pixel
            body = rounded_square(x, y, BODY_HALF, BODY_RADIUS)
            alpha = coverage(body)
            if alpha == 0.0:
                row += b"\x00\x00\x00\x00"
                continue

            t = min(max((y - (CENTER - BODY_HALF)) / (2 * BODY_HALF), 0.0), 1.0)
            color = [a + (b - a) * t for a, b in zip(PANEL_TOP, PANEL_BOTTOM)]

            def paint(layer, amount):
                for c in range(3):
                    color[c] += (layer[c] - color[c]) * amount

            # A hairline just inside the edge of the panel.
            paint(OUTLINE, coverage(abs(body + 0.012) - 0.004))

            inside = coverage(math.hypot(x - CENTER, y - CENTER) - (RING_RADIUS - RING_WIDTH))
            if inside > 0.0:
                # The earlier sighting: a fainter copy up and to the left.
                paint(FIGURE, 0.30 * inside * coverage(person(x, y, -0.085, -0.035)))
                # A dark gap separates the two figures.
                paint(PANEL_BOTTOM, inside * coverage(person(x, y, 0.04, 0.02) - 0.022))
                paint(FIGURE, inside * coverage(person(x, y, 0.04, 0.02)))
            paint(AMBER, coverage(reticle(x, y)))

            row += bytes(int(round(min(max(c, 0.0), 255.0))) for c in color)
            row.append(int(round(alpha * 255)))
        rows.append(bytes(row))
    return rows


def write_png(path, size, rows):
    def chunk(kind, data):
        body = kind + data
        return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body))

    raw = b"".join(b"\x00" + row for row in rows)
    with open(path, "wb") as f:
        f.write(b"\x89PNG\r\n\x1a\n")
        f.write(chunk(b"IHDR", struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0)))
        f.write(chunk(b"IDAT", zlib.compress(raw, 9)))
        f.write(chunk(b"IEND", b""))


def main():
    here = os.path.dirname(os.path.abspath(__file__))
    default_out = os.path.join(here, "..", "crates", "familiar-mac", "icons")
    ap = argparse.ArgumentParser(description="Draws the Familiar app icon")
    ap.add_argument("--out", default=default_out, help="output folder")
    args = ap.parse_args()

    out = os.path.abspath(args.out)
    os.makedirs(out, exist_ok=True)
    rendered = {}
    for points in POINT_SIZES:
        for scale, suffix in ((1, ""), (2, "@2x")):
            size = points * scale
            if size not in rendered:
                rendered[size] = render(size)
            path = os.path.join(out, f"icon_{points}x{points}{suffix}.png")
            write_png(path, size, rendered[size])
            print(f"{size:>5} px  {path}")

if __name__ == "__main__":
    main()
