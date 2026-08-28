#!/usr/bin/env python3
"""Generate all Tauri icon assets from the locked-in F icon design.

F design: white background, gold border, M/C teal, bolt gold.
The icon SVG is 256×256; we render it at the resolutions Tauri/Windows/macOS/Android/iOS expect.
"""
import gi
gi.require_version('Rsvg', '2.0')
from gi.repository import Rsvg
import os
import re
import sys
import struct
import zlib

ICON_SVG = '/home/adeal/.openclaw/workspace/projects/miracle-claw/brand/variants2/H-mcle-tight-icon.svg'
ICON_DIR = '/home/adeal/.openclaw/workspace/projects/miracle-claw/src-tauri/icons'

# All PNG sizes we need to generate.
# Names match what Tauri/Windows/macOS expect.
TARGETS = [
    # Linux/macOS/Windows cross-platform (in icons/ root)
    ("32x32.png", 32),
    ("64x64.png", 64),
    ("128x128.png", 128),
    ("128x128@2x.png", 256),
    ("icon.png", 512),
    # Windows Store square tiles (Tauri uses these for MSI/Store builds)
    ("Square30x30Logo.png", 30),
    ("Square44x44Logo.png", 44),
    ("Square71x71Logo.png", 71),
    ("Square89x89Logo.png", 89),
    ("Square107x107Logo.png", 107),
    ("Square142x142Logo.png", 142),
    ("Square150x150Logo.png", 150),
    ("Square284x284Logo.png", 284),
    ("Square310x310Logo.png", 310),
    # Windows Store badge
    ("StoreLogo.png", 50),
]


def render_at_size(svg_path, out_path, pixel_size):
    """Render an SVG to PNG at a specific pixel size using Rsvg."""
    with open(svg_path) as f:
        svg = f.read()
    # Inject explicit width/height to control rasterization size
    # The SVG has viewBox="0 0 256 256" with width="256" height="256".
    # Replace width/height attributes with the target size.
    svg = re.sub(r'width="\d+"', f'width="{pixel_size}"', svg, count=1)
    svg = re.sub(r'height="\d+"', f'height="{pixel_size}"', svg, count=1)
    tmp = f'/tmp/render_{pixel_size}.svg'
    with open(tmp, 'w') as f:
        f.write(svg)
    h = Rsvg.Handle.new_from_file(tmp)
    pix = h.get_pixbuf()
    pix.savev(out_path, 'png', [], [])


def make_ico(png_paths_and_sizes, out_path):
    """Build a multi-size Windows .ico file from PNG inputs.
    Each PNG must be square. The ICO format wraps them as PNG-with-alpha icons."""
    # ICONDIR: ICONDIR (6 bytes) + ICONDIRENTRY * N (16 bytes each)
    # ICONDIRENTRY: width(1), height(1), color_count(1), reserved(1), planes(2),
    #                bits_per_pixel(2), bytes_in_res(4), image_offset(4)
    n = len(png_paths_and_sizes)
    header = struct.pack('<HHH', 0, 1, n)
    entries_size = 16 * n
    offsets = []
    cur = 6 + entries_size
    png_data_list = []
    for png_path, size in png_paths_and_sizes:
        with open(png_path, 'rb') as f:
            data = f.read()
        png_data_list.append(data)
        offsets.append(cur)
        cur += len(data)
    entries = b''
    for (png_path, size), data, offset in zip(png_paths_and_sizes, png_data_list, offsets):
        w = size if size < 256 else 0  # 0 means 256
        h = size if size < 256 else 0
        entries += struct.pack('<BBBBHHII', w, h, 0, 0, 1, 32, len(data), offset)
    with open(out_path, 'wb') as f:
        f.write(header)
        f.write(entries)
        for data in png_data_list:
            f.write(data)


def make_icns(png_paths_and_sizes, out_path):
    """Build a macOS .icns file from PNG inputs.
    Each PNG must be square. The ICNS format wraps them as PNG-with-alpha icons."""
    # 'icns' magic + size(uint32) + chunks
    # Each chunk: '????' type(4) + size(uint32 BE) + data
    chunks = b''
    for png_path, size in png_paths_and_sizes:
        with open(png_path, 'rb') as f:
            data = f.read()
        # Pick the right ostype for this size
        if size == 16:    otype = b'icp4'
        elif size == 32:  otype = b'icp5'
        elif size == 64:  otype = b'icp6'
        elif size == 128: otype = b'ic07'
        elif size == 256: otype = b'ic08'
        elif size == 512: otype = b'ic09'
        elif size == 1024:otype = b'ic10'
        else:
            print(f"  WARN: skipping {png_path} size {size} (no icns type)")
            continue
        chunk = otype + struct.pack('>I', len(data) + 8) + data
        chunks += chunk
    total_size = 8 + len(chunks)
    with open(out_path, 'wb') as f:
        f.write(b'icns')
        f.write(struct.pack('>I', total_size))
        f.write(chunks)


def main():
    os.chdir(ICON_DIR)
    # Generate all PNGs from the F icon design
    rendered = {}  # filename -> (path, size)
    print(f"Rendering from {ICON_SVG}")
    for filename, size in TARGETS:
        out_path = os.path.join(ICON_DIR, filename)
        # Save a temporary intermediate at exact size
        render_at_size(ICON_SVG, out_path, size)
        rendered[filename] = (out_path, size)
        print(f"  → {filename} ({size}px)")

    # Generate android/
    android_sizes = {
        'android/mipmap-mdpi/ic_launcher.png': 48,
        'android/mipmap-hdpi/ic_launcher.png': 72,
        'android/mipmap-xhdpi/ic_launcher.png': 96,
        'android/mipmap-xxhdpi/ic_launcher.png': 144,
        'android/mipmap-xxxhdpi/ic_launcher.png': 192,
    }
    for filename, size in android_sizes.items():
        full = os.path.join(ICON_DIR, filename)
        os.makedirs(os.path.dirname(full), exist_ok=True)
        render_at_size(ICON_SVG, full, size)
        print(f"  → {filename} ({size}px)")

    # Generate iOS/
    ios_sizes = {
        'ios/AppIcon-20x20@1x.png': 20,
        'ios/AppIcon-20x20@2x.png': 40,
        'ios/AppIcon-20x20@2x-1.png': 40,
        'ios/AppIcon-20x20@3x.png': 60,
        'ios/AppIcon-29x29@1x.png': 29,
        'ios/AppIcon-29x29@2x.png': 58,
        'ios/AppIcon-29x29@2x-1.png': 58,
        'ios/AppIcon-29x29@3x.png': 87,
        'ios/AppIcon-40x40@1x.png': 40,
        'ios/AppIcon-40x40@2x.png': 80,
        'ios/AppIcon-40x40@2x-1.png': 80,
        'ios/AppIcon-40x40@3x.png': 120,
        'ios/AppIcon-60x60@2x.png': 120,
        'ios/AppIcon-60x60@3x.png': 180,
        'ios/AppIcon-76x76@1x.png': 76,
        'ios/AppIcon-76x76@2x.png': 152,
        'ios/AppIcon-83.5x83.5@2x.png': 167,
        'ios/AppIcon-512@2x.png': 1024,
    }
    for filename, size in ios_sizes.items():
        full = os.path.join(ICON_DIR, filename)
        render_at_size(ICON_SVG, full, size)
        print(f"  → {filename} ({size}px)")

    # Build icon.ico from a subset of PNG sizes (Windows wants 16/32/48/64/128/256)
    ico_sizes = [
        (os.path.join(ICON_DIR, '32x32.png'), 32),
        (os.path.join(ICON_DIR, '64x64.png'), 64),
        (os.path.join(ICON_DIR, '128x128.png'), 128),
        (os.path.join(ICON_DIR, 'icon.png'), 256),  # downscale by reference but we need a true 256px
    ]
    # Need an explicit 256px PNG for the ICO (icon.png is 512)
    render_at_size(ICON_SVG, '/tmp/icon_256.png', 256)
    ico_sizes.insert(3, ('/tmp/icon_256.png', 256))
    # Add 16px too
    render_at_size(ICON_SVG, '/tmp/icon_16.png', 16)
    ico_sizes.insert(0, ('/tmp/icon_16.png', 16))
    make_ico(ico_sizes, os.path.join(ICON_DIR, 'icon.ico'))
    print(f"  → icon.ico")

    # Build icon.icns from the macOS sizes (128/256/512/1024)
    icns_sizes = []
    for fn, sz in [('128x128.png', 128), ('icon.png', 256)]:
        path, _ = rendered[fn]
        icns_sizes.append((path, sz))
    # Add 512 and 1024 explicitly
    render_at_size(ICON_SVG, '/tmp/icon_512.png', 512)
    render_at_size(ICON_SVG, '/tmp/icon_1024.png', 1024)
    icns_sizes.append(('/tmp/icon_512.png', 512))
    icns_sizes.append(('/tmp/icon_1024.png', 1024))
    make_icns(icns_sizes, os.path.join(ICON_DIR, 'icon.icns'))
    print(f"  → icon.icns")

    print("\nAll icons generated.")


if __name__ == '__main__':
    main()
