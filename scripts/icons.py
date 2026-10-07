#!/usr/bin/env python3
"""Renders the icons from their SVG sources. Needs GdkPixbuf with librsvg
(python3-gi on Debian and Ubuntu). Run it after changing an SVG:

    python3 scripts/icons.py

- assets/icon/pastazzo.svg          → apple/macos/AppIcon.icns, assets/icon/pastazzo.png
- assets/icon/pastazzo-menubar.svg  → apple/macos/MenuBarIcon.png (1x)
- extension/icons/pastazzo-symbolic.svg → apple/macos/MenuBarIcon@2x.png
"""

import struct
from pathlib import Path

import gi

gi.require_version("GdkPixbuf", "2.0")
from gi.repository import GdkPixbuf  # noqa: E402

ROOT = Path(__file__).resolve().parent.parent
APP_ICON = ROOT / "assets/icon/pastazzo.svg"
MENU_BAR_1X = ROOT / "assets/icon/pastazzo-menubar.svg"
SYMBOLIC = ROOT / "extension/icons/pastazzo-symbolic.svg"
MACOS = ROOT / "apple/macos"

# The PNG entries of an .icns file, by pixel size, as iconutil writes them.
ICNS_TYPES = [
    (b"icp4", 16), (b"icp5", 32), (b"ic11", 32), (b"ic12", 64),
    (b"ic07", 128), (b"ic13", 256), (b"ic08", 256), (b"ic14", 512),
    (b"ic09", 512), (b"ic10", 1024),
]


def render(svg: Path, width: int, height: int) -> GdkPixbuf.Pixbuf:
    return GdkPixbuf.Pixbuf.new_from_file_at_scale(str(svg), width, height, False)


def png(pixbuf: GdkPixbuf.Pixbuf) -> bytes:
    ok, data = pixbuf.save_to_bufferv("png", [], [])
    assert ok
    return data


def write_icns(svg: Path, out: Path) -> None:
    chunks = b""
    for kind, size in ICNS_TYPES:
        data = png(render(svg, size, size))
        chunks += kind + struct.pack(">I", 8 + len(data)) + data
    out.write_bytes(b"icns" + struct.pack(">I", 8 + len(chunks)) + chunks)


def main() -> None:
    write_icns(APP_ICON, MACOS / "AppIcon.icns")
    render(APP_ICON, 256, 256).savev(str(ROOT / "assets/icon/pastazzo.png"), "png", [], [])

    render(MENU_BAR_1X, 17, 16).savev(str(MACOS / "MenuBarIcon.png"), "png", [], [])
    # The 16-point symbolic icon, centred on the same 17×16 point canvas.
    retina = GdkPixbuf.Pixbuf.new(GdkPixbuf.Colorspace.RGB, True, 8, 34, 32)
    retina.fill(0)
    render(SYMBOLIC, 32, 32).copy_area(0, 0, 32, 32, retina, 1, 0)
    retina.savev(str(MACOS / "MenuBarIcon@2x.png"), "png", [], [])


if __name__ == "__main__":
    main()
