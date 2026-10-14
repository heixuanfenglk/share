"""生成局域网文件分享图标：圆角蓝底 + 简化二维码定位点。"""

from __future__ import annotations

import struct
import zlib
from pathlib import Path

# 与界面 ACCENT 接近
BG = (46, 132, 196, 255)
FG = (255, 255, 255, 255)
DOT = (226, 245, 255, 255)


def png(width: int, height: int, rgba: bytes) -> bytes:
    raw = bytearray()
    stride = width * 4
    for y in range(height):
        raw.append(0)
        raw.extend(rgba[y * stride : (y + 1) * stride])

    def chunk(tag: bytes, data: bytes) -> bytes:
        return (
            struct.pack(">I", len(data))
            + tag
            + data
            + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)
        )

    ihdr = struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0)
    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", ihdr)
        + chunk(b"IDAT", zlib.compress(bytes(raw), 9))
        + chunk(b"IEND", b"")
    )


def ico(images: list[tuple[int, int, bytes]]) -> bytes:
    count = len(images)
    header = struct.pack("<HHH", 0, 1, count)
    entries = bytearray()
    blobs = bytearray()
    offset = 6 + 16 * count
    for width, height, blob in images:
        entries += struct.pack(
            "<BBBBHHII",
            0 if width >= 256 else width,
            0 if height >= 256 else height,
            0,
            0,
            1,
            32,
            len(blob),
            offset,
        )
        blobs += blob
        offset += len(blob)
    return header + bytes(entries) + bytes(blobs)


def rounded_cover(x: float, y: float, size: int, radius: float, inset: float) -> float:
    left = inset
    right = size - inset
    top = inset
    bottom = size - inset
    cx = min(max(x, left + radius), right - radius)
    cy = min(max(y, top + radius), bottom - radius)
    dx = x - cx
    dy = y - cy
    dist = (dx * dx + dy * dy) ** 0.5
    return max(0.0, min(1.0, radius - dist + 0.5))


def fill_rect(
    px: bytearray,
    size: int,
    x0: int,
    y0: int,
    x1: int,
    y1: int,
    color: tuple[int, int, int, int],
) -> None:
    x0 = max(0, min(size, x0))
    y0 = max(0, min(size, y0))
    x1 = max(0, min(size, x1))
    y1 = max(0, min(size, y1))
    if x0 >= x1 or y0 >= y1:
        return
    r, g, b, a = color
    for y in range(y0, y1):
        row = y * size * 4
        for x in range(x0, x1):
            i = row + x * 4
            px[i : i + 4] = bytes((r, g, b, a))


def finder(px: bytearray, size: int, ox: float, oy: float, cell: float) -> None:
    """QR 定位点：外框 + 内实心。"""
    outer = 7 * cell
    x0 = int(round(ox))
    y0 = int(round(oy))
    fill_rect(px, size, x0, y0, int(round(ox + outer)), int(round(oy + outer)), FG)
    fill_rect(
        px,
        size,
        int(round(ox + cell)),
        int(round(oy + cell)),
        int(round(ox + outer - cell)),
        int(round(oy + outer - cell)),
        BG,
    )
    fill_rect(
        px,
        size,
        int(round(ox + 2 * cell)),
        int(round(oy + 2 * cell)),
        int(round(ox + outer - 2 * cell)),
        int(round(oy + outer - 2 * cell)),
        FG,
    )


def draw_rgba(size: int) -> bytes:
    px = bytearray(size * size * 4)
    radius = size * 0.22
    inset = size * 0.04
    for y in range(size):
        for x in range(size):
            cover = rounded_cover(x + 0.5, y + 0.5, size, radius, inset)
            if cover <= 0:
                continue
            i = (y * size + x) * 4
            px[i : i + 4] = bytes((BG[0], BG[1], BG[2], int(round(BG[3] * cover))))

    # 内容区按 21 格二维码布局
    margin = size * 0.18
    grid = size - 2 * margin
    cell = grid / 21.0
    ox = margin
    oy = margin

    finder(px, size, ox, oy, cell)
    finder(px, size, ox + 14 * cell, oy, cell)
    finder(px, size, ox, oy + 14 * cell, cell)

    # 中间若干数据块，暗示可分享内容
    blocks = (
        (9, 9, 3, 3),
        (13, 10, 2, 2),
        (10, 13, 2, 3),
        (15, 15, 2, 2),
        (17, 9, 2, 1),
        (9, 17, 1, 2),
    )
    for bx, by, bw, bh in blocks:
        fill_rect(
            px,
            size,
            int(round(ox + bx * cell)),
            int(round(oy + by * cell)),
            int(round(ox + (bx + bw) * cell)),
            int(round(oy + (by + bh) * cell)),
            DOT if (bx + by) % 2 == 0 else FG,
        )
    return bytes(px)


def main() -> None:
    out = Path(__file__).resolve().parent
    sizes = (16, 24, 32, 48, 64, 128, 256)
    images = []
    png256 = b""
    for size in sizes:
        blob = png(size, size, draw_rgba(size))
        images.append((size, size, blob))
        if size == 256:
            png256 = blob
    (out / "icon.png").write_bytes(png256)
    (out / "icon.ico").write_bytes(ico(images))
    print(f"wrote {out / 'icon.ico'} and {out / 'icon.png'}")


if __name__ == "__main__":
    main()
