#!/usr/bin/env python3
"""Render a map the robot uploaded (`infoType` 20002) as a human-visible PNG.

Reads the newest `map_uploads` row from noobscenic's SQLite database — or a saved
20002 payload with `--json` — decompresses the grid and paints it:

    0x00 walls        dark
    0x7F unknown      mid grey
    0xFF free floor   white
    other bytes       room labels, one colour each (FUNC_MAP.md section 2.3)

The charging dock is marked with a red diamond when the upload carried one.
`--origin bottom` (the default) puts the robot's +y axis up; the wire's row 0 is
the smallest y, so the image is flipped to read like a floor plan.

Requires `pillow` and `lz4` (stdlib-only is the rule for the channel-C tools, but a
map renderer is a dev-machine tool):  python -m pip install pillow lz4
"""

from __future__ import annotations

import argparse
import base64
import colorsys
import json
import sqlite3
import sys
from pathlib import Path

import lz4.block
from PIL import Image, ImageDraw

WALL = (0x00, (28, 28, 36))
UNKNOWN = (0x7F, (158, 162, 172))
FREE = (0xFF, (250, 250, 246))
DOCK = (230, 40, 40)


class MapError(Exception):
    pass


def from_database(path: Path, sn: str | None) -> dict:
    if not path.exists():
        raise MapError("no database at %s (--db)" % path)
    db = sqlite3.connect("file:%s?mode=ro" % path.as_posix(), uri=True)
    try:
        query = (
            "SELECT sn, map_id, path_id, width, height, resolution, x_min, y_min, "
            "cells_lz4, dock_x, dock_y, dock_phi, dock_state, areas_json, trace_ref, "
            "received_ms FROM map_uploads"
        )
        if sn:
            row = db.execute(query + " WHERE sn = ? ORDER BY received_ms DESC LIMIT 1", (sn,)).fetchone()
        else:
            row = db.execute(query + " ORDER BY received_ms DESC LIMIT 1").fetchone()
    finally:
        db.close()
    if row is None:
        raise MapError("no map uploads in %s%s" % (path, " for %s" % sn if sn else ""))
    keys = [
        "sn", "map_id", "path_id", "width", "height", "resolution", "x_min", "y_min",
        "cells_lz4", "dock_x", "dock_y", "dock_phi", "dock_state", "areas_json",
        "trace_ref", "received_ms",
    ]
    return dict(zip(keys, row))


def from_json(path: Path) -> dict:
    payload = json.loads(path.read_text(encoding="utf-8"))
    if payload.get("infoType") != 20002:
        raise MapError("%s does not contain an infoType 20002 payload" % path)
    data = payload["data"]
    pos = data.get("chargeHandlePos") or []
    return {
        "sn": data.get("SN"),
        "map_id": data.get("mapId"),
        "path_id": data.get("pathId"),
        "width": data["width"],
        "height": data["height"],
        "resolution": data.get("resolution"),
        "x_min": data.get("x_min"),
        "y_min": data.get("y_min"),
        "cells_lz4": base64.b64decode(data["map"]),
        "dock_x": pos[0] if len(pos) == 2 else None,
        "dock_y": pos[1] if len(pos) == 2 else None,
        "dock_phi": data.get("chargeHandlePhi"),
        "dock_state": data.get("chargeHandleState"),
        "areas_json": json.dumps(data.get("area", [])),
        "trace_ref": str(path),
        "received_ms": None,
    }


def cells(upload: dict) -> bytes:
    width, height = int(upload["width"]), int(upload["height"])
    expected = width * height
    try:
        grid = lz4.block.decompress(upload["cells_lz4"], uncompressed_size=expected)
    except Exception as exc:  # lz4.block.LZ4BlockError and friends
        raise MapError("LZ4 block did not decompress: %s" % exc)
    if len(grid) != expected:
        raise MapError("grid is %d bytes, expected %d (%dx%d)" % (len(grid), expected, width, height))
    return grid


def label_colour(label: int) -> tuple[int, int, int]:
    # Labels are room ids; a stable hue per id keeps renders comparable.
    hue = (label * 0.61803398875) % 1.0
    r, g, b = colorsys.hsv_to_rgb(hue, 0.42, 0.96)
    return int(r * 255), int(g * 255), int(b * 255)


def render(upload: dict, grid: bytes, scale: int, origin: str) -> tuple[Image.Image, dict[int, int]]:
    width, height = int(upload["width"]), int(upload["height"])
    palette = {WALL[0]: WALL[1], UNKNOWN[0]: UNKNOWN[1], FREE[0]: FREE[1]}
    counts: dict[int, int] = {}
    rows = []
    for row in range(height):
        pixels = bytearray()
        for cell in grid[row * width : (row + 1) * width]:
            counts[cell] = counts.get(cell, 0) + 1
            colour = palette.get(cell) or label_colour(cell)
            pixels += bytes(colour)
        rows.append(bytes(pixels))
    if origin == "bottom":
        rows.reverse()

    image = Image.frombytes("RGB", (width, height), b"".join(rows))
    if scale != 1:
        image = image.resize((width * scale, height * scale), Image.NEAREST)

    if upload.get("dock_x") is not None and upload.get("dock_y") is not None:
        resolution = float(upload["resolution"] or 0) or 0.05
        # Cell centres sit half a cell inside the wire origin (MAP.md).
        col = (upload["dock_x"] / 1000.0 - float(upload["x_min"] or 0) - 0.05) / resolution
        row = (upload["dock_y"] / 1000.0 - float(upload["y_min"] or 0) - 0.05) / resolution
        if origin == "bottom":
            row = height - 1 - row
        x, y = (col + 0.5) * scale, (row + 0.5) * scale
        radius = max(2.0, scale * 1.6)
        draw = ImageDraw.Draw(image)
        draw.polygon([(x, y - radius), (x + radius, y), (x, y + radius), (x - radius, y)], fill=DOCK)

    return image, counts


def describe(upload: dict, counts: dict[int, int]) -> None:
    print("map %s  path %s" % (upload.get("map_id"), upload.get("path_id")))
    print(
        "  %dx%d cells, %s m/cell, origin (%s, %s) mm"
        % (
            upload["width"],
            upload["height"],
            upload.get("resolution"),
            upload.get("x_min"),
            upload.get("y_min"),
        )
    )
    print("  trace %s" % upload.get("trace_ref"))
    named = {}
    try:
        for area in json.loads(upload.get("areas_json") or "[]"):
            named[area.get("id")] = area.get("name")
    except (TypeError, ValueError):
        pass
    for cell in sorted(counts):
        if cell == WALL[0]:
            what = "wall"
        elif cell == UNKNOWN[0]:
            what = "unknown"
        elif cell == FREE[0]:
            what = "free"
        else:
            what = "label %d%s" % (cell, " (%s)" % named[cell] if cell in named else "")
        print("  %-24s %6d cells" % (what, counts[cell]))
    if upload.get("dock_x") is not None:
        print("  dock (%s, %s) mm, state %s" % (upload["dock_x"], upload["dock_y"], upload.get("dock_state")))


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--db", default="var/noobscenic.db", help="SQLite database (default %(default)s)")
    parser.add_argument("--sn", help="only consider this device serial")
    parser.add_argument("--json", help="read a saved 20002 payload instead of the database")
    parser.add_argument("--out", default="map.png", help="PNG to write (default %(default)s)")
    parser.add_argument("--scale", type=int, default=4, help="pixels per cell (default %(default)s)")
    parser.add_argument(
        "--origin",
        choices=("bottom", "top"),
        default="bottom",
        help="which edge row 0 sits on; bottom puts +y up (default %(default)s)",
    )
    args = parser.parse_args(argv)

    try:
        upload = from_json(Path(args.json)) if args.json else from_database(Path(args.db), args.sn)
        grid = cells(upload)
        image, counts = render(upload, grid, args.scale, args.origin)
    except MapError as exc:
        print("error: %s" % exc, file=sys.stderr)
        return 1

    image.save(args.out)
    describe(upload, counts)
    print("wrote %s (%dx%d px)" % (args.out, image.width, image.height))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
