# 卡通风格应用图标:黄色圆角底 + 粉色小猪存钱罐 + 掉进投币口的 ¥ 金币。
# 纯标准库,用有向距离场画形状,粗描边 + 抗锯齿。
import math, struct, sys, zlib

N = 1024
OL = 11  # 描边半宽
INK = (0x4a, 0x2c, 0x17)

def cov(d):
    return max(0.0, min(1.0, 0.5 - d))

def rrect(cx, cy, hw, hh, r):
    def f(x, y):
        qx, qy = abs(x - cx) - hw + r, abs(y - cy) - hh + r
        return math.hypot(max(qx, 0), max(qy, 0)) + min(max(qx, qy), 0) - r
    return f

def ellipse(cx, cy, rx, ry, ang=0.0):
    ca, sa = math.cos(math.radians(ang)), math.sin(math.radians(ang))
    def f(x, y):
        dx, dy = x - cx, y - cy
        px, py = dx * ca + dy * sa, -dx * sa + dy * ca
        k0 = math.hypot(px / rx, py / ry)
        k1 = math.hypot(px / (rx * rx), py / (ry * ry))
        return -min(rx, ry) if k1 == 0 else k0 * (k0 - 1) / k1
    return f

def circle(cx, cy, r):
    return lambda x, y: math.hypot(x - cx, y - cy) - r

def capsule(ax, ay, bx, by, w):
    dx, dy = bx - ax, by - ay
    L = dx * dx + dy * dy
    def f(x, y):
        t = 0 if L == 0 else max(0, min(1, ((x - ax) * dx + (y - ay) * dy) / L))
        return math.hypot(x - ax - t * dx, y - ay - t * dy) - w
    return f

def union(*fs):
    return lambda x, y: min(f(x, y) for f in fs)

# 图层:(形状, 包围盒, 填充色 或 按 y 取色的函数, 是否描边, 不透明度)
def L(f, box, fill, outline=True, alpha=1.0):
    return (f, box, fill, outline, alpha)

PINK, PINK_D, PINK_L = (0xff, 0xa6, 0xbd), (0xff, 0x80, 0x9f), (0xff, 0xd3, 0xdf)
GOLD, GOLD_L, GOLD_D = (0xff, 0xb3, 0x1a), (0xff, 0xd9, 0x5c), (0xb8, 0x6a, 0x00)

def bg(_x, y):  # 上浅下深的暖黄
    t = (y - 100) / 824
    a, b = (0xff, 0xe2, 0x6e), (0xff, 0xc8, 0x1a)
    return tuple(round(a[i] + (b[i] - a[i]) * t) for i in range(3))

LAYERS = [
    L(rrect(512, 512, 412, 412, 185), (100, 100, 924, 924), bg, outline=False),
    # 金币(先画,下半截被小猪身体挡住,像正在投进去)
    L(circle(512, 292, 96), (400, 180, 624, 400), GOLD),
    L(circle(512, 292, 70), (430, 210, 594, 375), GOLD_L, outline=False),
    L(union(capsule(484, 254, 512, 292, 11), capsule(540, 254, 512, 292, 11), capsule(512, 292, 512, 336, 11),
            capsule(490, 300, 534, 300, 9), capsule(490, 322, 534, 322, 9)), (465, 235, 560, 352), GOLD_D, outline=False),
    # 闪光
    L(union(capsule(668, 214, 668, 262, 9), capsule(644, 238, 692, 238, 9)), (630, 200, 706, 276), (0xff, 0xff, 0xff), outline=False),
    L(union(capsule(352, 250, 352, 278, 7), capsule(338, 264, 366, 264, 7)), (325, 237, 380, 292), (0xff, 0xff, 0xff), outline=False),
    # 脚、耳朵(在身体后面)
    L(rrect(420, 862, 46, 40, 28), (360, 810, 480, 915), PINK_D),
    L(rrect(604, 862, 46, 40, 28), (544, 810, 664, 915), PINK_D),
    L(ellipse(318, 432, 62, 92, -38), (220, 320, 420, 545), PINK),
    L(ellipse(706, 432, 62, 92, 38), (604, 320, 804, 545), PINK),
    L(ellipse(326, 444, 30, 52, -38), (275, 380, 380, 510), PINK_D, outline=False),
    L(ellipse(698, 444, 30, 52, 38), (644, 380, 750, 510), PINK_D, outline=False),
    # 身体
    L(ellipse(512, 610, 300, 248), (195, 345, 829, 875), PINK),
    L(ellipse(420, 500, 120, 60, -18), (290, 430, 550, 575), PINK_L, outline=False, alpha=0.65),
    # 投币口
    L(capsule(462, 392, 562, 392, 13), (440, 370, 585, 415), INK, outline=False),
    # 眼睛 + 高光
    L(circle(410, 572, 30), (375, 537, 445, 607), INK, outline=False),
    L(circle(614, 572, 30), (579, 537, 649, 607), INK, outline=False),
    L(circle(400, 561, 10), (388, 549, 412, 573), (0xff, 0xff, 0xff), outline=False),
    L(circle(604, 561, 10), (592, 549, 616, 573), (0xff, 0xff, 0xff), outline=False),
    # 腮红
    L(ellipse(344, 660, 44, 26), (295, 630, 393, 690), (0xff, 0x6b, 0x8e), outline=False, alpha=0.55),
    L(ellipse(680, 660, 44, 26), (631, 630, 729, 690), (0xff, 0x6b, 0x8e), outline=False, alpha=0.55),
    # 鼻子
    L(ellipse(512, 680, 98, 70), (400, 595, 624, 765), PINK_D),
    L(ellipse(476, 680, 15, 24), (458, 653, 494, 707), INK, outline=False),
    L(ellipse(548, 680, 15, 24), (530, 653, 566, 707), INK, outline=False),
    # 微笑
    L(lambda x, y: (math.hypot(x - 512, y - 742) - 44 if y > 742 else 1e9) if True else 0, (0, 0, 0, 0), INK, outline=False),
]
# 微笑:下半圆弧,用圆环的下半部分
def smile(x, y):
    cy, r, w = 774, 26, 7
    d = abs(math.hypot(x - 512, y - cy) - r) - w
    if y < cy:
        d = min(math.hypot(x - 512 + r, y - cy), math.hypot(x - 512 - r, y - cy)) - w
    return d
LAYERS[-1] = L(smile, (470, 760, 555, 815), INK, outline=False)

def over(dst, src, a):
    return [dst[i] * (1 - a) + src[i] * a for i in range(3)], a

rows = []
for y in range(N):
    row = bytearray([0])
    py = y + 0.5
    for x in range(N):
        px = x + 0.5
        rgb, A = [0.0, 0.0, 0.0], 0.0
        for f, (x0, y0, x1, y1), fill, outline, alpha in LAYERS:
            if not (x0 <= px <= x1 and y0 <= py <= y1):
                continue
            d = f(px, py)
            if d > OL + 1:
                continue
            col = fill(px, py) if callable(fill) else fill
            if outline:
                a = cov(d - OL) * alpha
                if a > 0:
                    rgb = [rgb[i] * (1 - a) + INK[i] * a for i in range(3)]; A = A + a * (1 - A)
                a = cov(d + OL) * alpha
            else:
                a = cov(d) * alpha
            if a > 0:
                rgb = [rgb[i] * (1 - a) + col[i] * a for i in range(3)]; A = A + a * (1 - A)
        # 只在底板内着色:颜色是"预乘"混合的结果,按覆盖率还原
        if A <= 0:
            row += b"\0\0\0\0"
        else:
            row += bytes([min(255, round(c)) for c in rgb] + [round(A * 255)])
    rows.append(bytes(row))

def chunk(t, d):
    return struct.pack(">I", len(d)) + t + d + struct.pack(">I", zlib.crc32(t + d) & 0xffffffff)

open(sys.argv[1], "wb").write(b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", N, N, 8, 6, 0, 0, 0))
    + chunk(b"IDAT", zlib.compress(b"".join(rows), 9)) + chunk(b"IEND", b""))
