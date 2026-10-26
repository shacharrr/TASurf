"""Draw the TASurf app icon and write every size Tauri wants (needs Pillow):

    python rust/app/icons/make_icons.py

A dark tile with a run's path: red up to the cursor (the yellow dot), white after
it, as the game draws the path at the editor's tick.
"""

import math
from pathlib import Path

from PIL import Image, ImageDraw

HERE = Path(__file__).resolve().parent
S = 2048  # drawn at 2x of the largest size, then scaled down for smooth edges


def draw() -> Image.Image:
    img = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    # background: rounded tile with a vertical gradient
    grad = Image.new("RGBA", (S, S))
    gd = ImageDraw.Draw(grad)
    for y in range(S):
        t = y / (S - 1)
        c = tuple(round(a + (b - a) * t) for a, b in zip((32, 42, 60), (14, 18, 27)))
        gd.line([(0, y), (S, y)], fill=c + (255,))
    mask = Image.new("L", (S, S), 0)
    ImageDraw.Draw(mask).rounded_rectangle([64, 64, S - 64, S - 64], radius=420, fill=255)
    img.paste(grad, (0, 0), mask)
    d = ImageDraw.Draw(img)

    # a surf ramp: two faint slanted faces
    d.polygon([(300, 1500), (900, 900), (1000, 1650)], fill=(70, 88, 120, 255))
    d.polygon([(1000, 1650), (900, 900), (1500, 1100)], fill=(52, 66, 92, 255))

    # the path: a strafing curve across the tile, stamped as circles for a clean stroke
    n = 3000
    pts = []
    for i in range(n + 1):
        t = i / n
        x = 280 + t * (S - 560)
        y = 1300 - 560 * t + 150 * math.sin(t * math.pi * 3.2)
        pts.append((x, y))
    cut = round(n * 0.57)
    r = 60
    for i, (x, y) in enumerate(pts):
        col = (255, 72, 72, 255) if i <= cut else (245, 245, 245, 255)
        d.ellipse([x - r, y - r, x + r, y + r], fill=col)
    cx, cy = pts[cut]
    r = 150
    d.ellipse([cx - r, cy - r, cx + r, cy + r], fill=(255, 214, 64, 255), outline=(20, 24, 33, 255), width=36)
    return img


def main():
    big = draw()
    sized = lambda n: big.resize((n, n), Image.LANCZOS)
    sized(32).save(HERE / "32x32.png")
    sized(128).save(HERE / "128x128.png")
    sized(256).save(HERE / "128x128@2x.png")
    sized(512).save(HERE / "icon.png")
    sized(256).save(HERE / "icon.ico", sizes=[(16, 16), (24, 24), (32, 32), (48, 48), (64, 64), (128, 128), (256, 256)])
    sized(1024).save(HERE / "icon.icns")
    print("wrote", ", ".join(sorted(p.name for p in HERE.iterdir() if p.suffix != ".py")))


if __name__ == "__main__":
    main()
