"""Owlfice 픽셀아트 원본 생성기. 손으로 그림을 고치지 말고 이 파일을 고친다.

한 번 실행하면 아래를 모두 다시 만든다.
  sprites/owls.png  owls.json     올빼미 부품 시트 (몸통 · 옷 · 담당 소품, 자세별 프레임)
  sprites/office.png office.json  벽 · 바닥 타일 · 가구 · 소품 · 효과 시트 (상태별 프레임)
  sprites/ui.png    ui.json       로고 · 로딩 건물 부품 · 규모 선택 미리보기
  sprites/floorplan.json          평면도: 방, 배치, 좌석, 서 있을 자리, 통로, 테마
  sprites/preview/*.png           확인용 미리보기 (올빼미 20종, 자세, 레이어, 소품 목록)
  assets/*.png                    캔버스 시안용 참조 렌더. 위 시트와 평면도만으로 그린다.

참조 렌더(render_scene)는 앱의 PixiJS 렌더러가 따라야 할 그리기 순서 그대로다.
사용: python design/tools/owlfice_art.py
"""
import json
import math
import os

from PIL import Image, ImageDraw, ImageFont, ImageOps

HERE = os.path.dirname(os.path.abspath(__file__))
DESIGN = os.path.dirname(HERE)
SPR = os.path.join(DESIGN, "sprites")
PREV = os.path.join(SPR, "preview")
CANVAS = os.path.join(DESIGN, "assets")

# ---------- 좌표계 ----------
W, H = 520, 320          # 1x 캔버스
OX, OY = 212, 60         # 격자 (0,0) 꼭짓점의 화면 좌표
TILE_W, TILE_H = 24, 12
COLS, ROWS = 24, 16
WALL_H = 48


def iso(gx, gy, z=0):
    return (OX + (gx - gy) * 12, OY + (gx + gy) * 6 - z)


def rnd(p):
    return (int(round(p[0])), int(round(p[1])))


def shade(c, f):
    return tuple(max(0, min(255, int(v * f))) for v in c[:3])


def hexc(c):
    return "#%02X%02X%02X" % tuple(c[:3])


OUTLINE = (42, 30, 26)

THEMES = {
    "claude": dict(label="Claude", wall=(243, 205, 172), stripe=(236, 192, 156), base=(196, 120, 80),
                   uni=(217, 119, 63), uni_d=(170, 86, 42), boss_carpet=(226, 166, 128),
                   cubicle=(214, 196, 176), sofa=(176, 92, 60), accent=(217, 119, 69)),
    "codex": dict(label="Codex", wall=(232, 232, 230), stripe=(222, 222, 220), base=(120, 120, 120),
                  uni=(52, 52, 56), uni_d=(28, 28, 30), boss_carpet=(176, 176, 178),
                  cubicle=(206, 206, 206), sofa=(70, 70, 74), accent=(39, 39, 42)),
    "gemini": dict(label="Gemini", wall=(198, 216, 246), stripe=(184, 204, 240), base=(86, 120, 200),
                   uni=(59, 111, 224), uni_d=(38, 78, 176), boss_carpet=(160, 188, 236),
                   cubicle=(196, 208, 230), sofa=(60, 90, 170), accent=(61, 111, 224)),
}
TEAM_TINTS = [(52, 150, 150), (128, 98, 190), (226, 112, 92), (214, 160, 40), (70, 130, 200), (150, 110, 80)]
NIGHT_TINT = (242, 222, 173)  # 곱하기. 창문 레이어에는 적용하지 않는다
EVENING_TINT = (246, 230, 196)

ROOMS = {  # key: (이름, gx0, gy0, gx1, gy1, 바닥 타일)
    "library": ("소스 서고", 0, 0, 4, 5, "library"),
    "archive": ("자료실", 4, 0, 6, 5, "archive"),
    "build": ("빌드실", 6, 0, 11, 5, "build"),
    "test": ("테스트실", 11, 0, 16, 5, "test"),
    "boss": ("팀장실", 16, 0, 24, 6, "boss"),
    "dev": ("개발실", 0, 5, 16, 11, "dev"),
    "review": ("리뷰실", 16, 6, 24, 9, "review"),
    "incident": ("장애 상황실", 16, 9, 24, 12, "incident"),
    "conference": ("회의실", 16, 12, 20, 16, "conference"),
    "meeting": ("거래처 미팅실", 20, 12, 24, 16, "meeting"),
    "security": ("경비실", 0, 11, 5, 13, "security"),
    "entrance": ("출입문 · 출고장", 0, 13, 5, 16, "entrance"),
    "pantry": ("탕비실", 5, 11, 13, 16, "pantry"),
    "roof": ("옥상", 13, 11, 16, 16, "roof"),
}
FLOOR = {"library": (196, 160, 120), "build": (184, 188, 194), "test": (204, 208, 212), "dev": (218, 211, 198),
         "review": (214, 196, 170), "incident_off": (112, 108, 104), "incident_on": (190, 128, 118),
         "meeting": (196, 210, 188), "entrance": (190, 182, 170), "pantry": (234, 226, 210)}
FLOOR.update(archive=(174, 158, 136), conference=(186, 204, 218), security=(172, 182, 176), roof=(164, 176, 178))
for _t, _v in THEMES.items():
    FLOOR["boss_" + _t] = _v["boss_carpet"]


def room_at(i, j):
    for k, (_, a, b, c, e, _) in ROOMS.items():
        if a <= i < c and b <= j < e:
            return k
    return None


# ---------- 공통 그리기 ----------
class Painter:
    """소품 하나를 자기 캔버스에 그린다. 발자국의 (0,0,0) 꼭짓점이 기준점(anchor)이다."""

    def __init__(self, w, d, h, pad=3):
        self.cw = int(math.ceil((w + d) * 12)) + pad * 2 + 2
        self.ch = int(math.ceil((w + d) * 6)) + h + pad * 2 + 2
        self.ax = int(math.ceil(d * 12)) + pad + 1
        self.ay = h + pad + 1
        self.img = Image.new("RGBA", (self.cw, self.ch), (0, 0, 0, 0))
        self.d = ImageDraw.Draw(self.img)

    def iso(self, gx, gy, z=0):
        return (self.ax + (gx - gy) * 12, self.ay + (gx + gy) * 6 - z)

    def P(self, gx, gy, z=0):
        return rnd(self.iso(gx, gy, z))

    def poly(self, pts, fill, outline=None):
        self.d.polygon([self.P(*p) for p in pts], fill=fill, outline=outline)

    def box(self, gx, gy, w, dp, h, color, z=0, outline=True, top=None):
        ol = shade(color, 0.5) if outline else None
        self.poly([(gx, gy + dp, z), (gx + w, gy + dp, z), (gx + w, gy + dp, z + h), (gx, gy + dp, z + h)], shade(color, 0.86), ol)
        self.poly([(gx + w, gy, z), (gx + w, gy + dp, z), (gx + w, gy + dp, z + h), (gx + w, gy, z + h)], shade(color, 0.72), ol)
        self.poly([(gx, gy, z + h), (gx + w, gy, z + h), (gx + w, gy + dp, z + h), (gx, gy + dp, z + h)], top or color, ol)

    def lq(self, gx, gy, w, dp, z, u0, u1, v0, v1, color):  # +gy 면 (왼쪽 앞면)
        y = gy + dp
        self.poly([(gx + u0, y, z + v0), (gx + u1, y, z + v0), (gx + u1, y, z + v1), (gx + u0, y, z + v1)], color)

    def rq(self, gx, gy, w, dp, z, u0, u1, v0, v1, color):  # +gx 면 (오른쪽 앞면)
        x = gx + w
        self.poly([(x, gy + u0, z + v0), (x, gy + u1, z + v0), (x, gy + u1, z + v1), (x, gy + u0, z + v1)], color)

    def line(self, a, b, color):
        self.d.line([self.P(*a), self.P(*b)], fill=color)

    def dot(self, g, color, size=1):
        x, y = self.P(*g)
        self.d.rectangle([x, y, x + size - 1, y + size - 1], fill=color)

    def paste(self, spr, x, y):
        self.img.alpha_composite(spr, (int(round(x)), int(round(y))))

    def result(self):
        bb = self.img.getbbox() or (0, 0, 1, 1)
        return self.img.crop(bb), (self.ax - bb[0], self.ay - bb[1])


def pix(rows, pal):
    img = Image.new("RGBA", (max(len(r) for r in rows), len(rows)), (0, 0, 0, 0))
    for y, row in enumerate(rows):
        for x, ch in enumerate(row):
            if ch not in ". ":
                c = pal[ch]
                img.putpixel((x, y), c if len(c) == 4 else tuple(c) + (255,))
    return img


def tinted(img, tint):
    r, g, b, a = img.split()
    r = r.point(lambda v: v * tint[0] // 255)
    g = g.point(lambda v: v * tint[1] // 255)
    b = b.point(lambda v: v * tint[2] // 255)
    return Image.merge("RGBA", (r, g, b, a))


# =====================================================================
# 올빼미
# =====================================================================
OWL_W, OWL_H = 16, 24
OWL_AX, OWL_AY = 8, 23   # 발 기준점. 이 픽셀의 왼쪽 위 모서리가 발 좌표에 온다
TX, TY = 1, 6            # 템플릿 좌표 -> 프레임 좌표

SPECIES = {
    "eagle": dict(name="수리부엉이", top=0, lid="l", lidrow=6, eyes=[(3, 4), (9, 10)], rows=[
        ".kk........kk.", ".kbk......kbk.", ".kbbkkkkkkbbk.", "kbbbbbbbbbbbbk", "kbllllbbllllbk",
        "kbliilbbliilbk", "kbliplbblpilbk", "kblllloollllbk", "kbbbbboobbbbbk"],
        pal=dict(b=(139, 94, 60), B=(107, 70, 48), d=(107, 70, 48), l=(236, 214, 178), i=(240, 140, 40),
                 o=(232, 163, 58), c=(214, 186, 146))),
    "longeared": dict(name="칡부엉이", top=-2, lid="L", lidrow=6, eyes=[(3, 4), (9, 10)], rows=[
        "..k........k..", "..kk......kk..", "..kbk....kbk..", ".kbbk....kbbk.", ".kbbbkkkkbbbk.",
        "kbbbbbbbbbbbbk", "kbLLLLbbLLLLbk", "kbLiiLbbLiiLbk", "kbLipLbbLpiLbk", "kbLLLLooLLLLbk",
        "kbbbbboobbbbbk"],
        pal=dict(b=(152, 98, 58), B=(110, 70, 42), d=(110, 70, 42), L=(224, 142, 80), i=(250, 160, 40),
                 o=(70, 56, 44), c=(214, 170, 120))),
    "little": dict(name="금눈쇠올빼미", top=2, lid="w", lidrow=6, eyes=[(3, 4), (9, 10)], rows=[
        "...kkkkkkkk...", ".kbBbbbbbbBbk.", "kbwwwwbbwwwwbk", "kbwiiwbbwiiwbk", "kbBipbbbbpiBbk",
        "kbbbbboobbbbbk", "kbBbbbbbbbbBbk"],
        pal=dict(b=(132, 112, 90), B=(96, 80, 62), d=(96, 80, 62), w=(246, 242, 230), i=(250, 214, 60),
                 o=(214, 190, 120), c=(226, 214, 190))),
    "barn": dict(name="가면올빼미", top=2, lid="w", lidrow=5, eyes=[(3, 4), (9, 10)], rows=[
        "...kkkkkkkk...", "..kbbbbbbbbk..", ".kbFwwwwwwFbk.", "kbFppwwwwppFbk", "kbFwwwwwwwwFbk",
        ".kbFwwoowwFbk.", "..kbFFFFFFbk.."],
        pal=dict(b=(214, 166, 100), B=(170, 124, 70), d=(170, 124, 70), F=(184, 128, 72), w=(250, 246, 236),
                 o=(226, 196, 166), c=(246, 236, 214))),
    "scops": dict(name="소쩍새", top=1, lid="l", lidrow=6, eyes=[(3, 4), (9, 10)], rows=[
        "..k........k..", ".kbkkkkkkkkbk.", "kbbbbbbbbbbbbk", "kbllllbbllllbk", "kbliilbbliilbk",
        "kbliplbblpilbk", "kblllloollllbk", "kbBbbboobbbBbk"],
        pal=dict(b=(132, 130, 124), B=(96, 94, 90), d=(96, 94, 90), l=(194, 190, 180), i=(250, 210, 60),
                 o=(200, 170, 110), c=(204, 200, 190))),
    "snowy": dict(name="흰올빼미", top=2, lid="b", lidrow=6, eyes=[(3, 4), (9, 10)], rows=[
        "...kkkkkkkk...", "..kbbbbBbbbk..", ".kbbbbbbbbbbk.", "kbbiibbbbiibbk", "kbbipbbbbpibbk",
        "kbBbbboobbbbBk", "kbbbbBbbbBbbbk"],
        pal=dict(b=(242, 240, 232), B=(70, 70, 70), d=(214, 212, 204), i=(250, 210, 60), o=(70, 66, 60),
                 c=(250, 250, 246))),
}
EMPLOYEE_SPECIES = ["eagle", "longeared", "little", "barn", "scops"]  # 흰올빼미는 사장님 전용
BODY_PAL = dict(k=OUTLINE, p=(26, 26, 26), y=(232, 163, 58), m=(244, 236, 212), M=(110, 70, 42))

TORSO = ["kdccccccccccdk", "kdccccccccccdk", "kdccccccccccdk", "kddccccccccddk", ".kdccccccccdk.",
         ".kdccccccccdk.", "..kkcccccckk..", "...kkkkkkkk..."]   # 템플릿 9~16행
WING_DEFAULT = {(0, 9): "k", (0, 10): "k", (0, 11): "k", (0, 12): "k", (1, 9): "d", (1, 10): "d", (1, 11): "d",
                (1, 12): "d", (2, 12): "d", (1, 13): "k", (2, 13): "d", (1, 14): "k", (2, 14): "d"}
WING_ERASED = {(0, 9): None, (0, 10): None, (0, 11): None, (0, 12): None, (1, 9): "k", (1, 10): "k",
               (1, 11): "k", (1, 12): "k", (2, 12): "c", (1, 13): None, (2, 13): "k", (1, 14): None, (2, 14): "k"}
WING_BLOBS = {   # 왼쪽 날개 모양 (템플릿 좌표, 몸통 기준). 오른쪽은 x -> 13-x
    "fwd": [(2, 11, "k"), (3, 11, "d"), (4, 11, "d"), (5, 11, "k"), (3, 12, "k"), (4, 12, "k")],
    "clapout": [(3, 10, "k"), (4, 10, "d"), (5, 10, "d"), (6, 10, "k"), (4, 11, "k"), (5, 11, "k")],
    "clapin": [(4, 10, "k"), (5, 10, "d"), (6, 10, "d"), (7, 10, "k"), (5, 11, "k"), (6, 11, "k")],
    "up": [(-1, 4, "k"), (-1, 5, "k"), (-1, 6, "k"), (-1, 7, "k"), (0, 3, "k"), (0, 4, "d"), (0, 5, "d"),
           (0, 6, "d"), (0, 7, "d"), (0, 8, "k")],
    "head": [(1, 1, "k"), (2, 1, "k"), (0, 2, "k"), (1, 2, "d"), (2, 2, "d"), (3, 2, "k"), (0, 3, "k"),
             (1, 3, "d"), (2, 3, "k"), (1, 4, "k")],
    "chin": [(3, 9, "k"), (4, 8, "k"), (5, 8, "d"), (6, 8, "k"), (4, 9, "d"), (5, 9, "k"), (3, 10, "k")],
}
HEAD_BLOBS = {"up", "head"}   # 머리 기준으로 움직이는 날개

# 자세: dir별 프레임 수, fps, 반복 여부
POSES = {
    "idle": dict(f=2, b=1, fps=0, loop=True, note="서 있기. f의 1번은 눈 깜빡임: 3~5초마다 150ms"),
    "walk": dict(f=4, b=4, fps=8, loop=True, note="걷기. 발 기준 0.5~1.5초에 방 사이를 이동"),
    "sit": dict(f=1, fps=0, loop=False, note="앉기. 책상 뒤, 소파, 안마의자"),
    "type": dict(f=2, fps=6, loop=True, note="파일 수정·작성"),
    "read": dict(f=2, fps=2, loop=True, note="파일 열람. 마우스 휠"),
    "sleep": dict(f=2, fps=1, loop=True, note="졸음. 탕비실 소파·안마의자"),
    "jump": dict(f=1, fps=0, loop=False, note="펄쩍 (사장님 등장). 0.4초"),
    "scratch": dict(f=2, fps=4, loop=True, note="머리 긁기 (반려, 같은 파일 3회 수정)"),
    "panic": dict(f=2, fps=6, loop=True, note="머리 쥐어뜯기 (장애 상황실)"),
    "cheer": dict(f=1, fps=0, loop=False, note="만세 (테스트 전부 통과)"),
    "clap": dict(f=2, fps=6, loop=True, note="손뼉 (서브에이전트 지시)"),
    "crouch": dict(f=1, fps=0, loop=False, note="쪼그려 앉기 (명령이 평균 2배 초과)"),
    "carry": dict(f=1, b=1, fps=0, loop=False, note="앞으로 들기 (결재판, 서류철, 러버덕, 상자)"),
    "think": dict(f=2, fps=1, loop=True, note="턱 괴기. 기존 침묵 추정 자세"),
    "highfive": dict(f=1, fps=0, loop=False, note="같은 팀 두 직원 확정 통과. 동시에 1초"),
    "stretch": dict(f=2, fps=2, loop=True, note="근속 1시간 경계마다 기지개 1.5초"),
    "drink": dict(f=2, fps=2, loop=True, note="응답 대기 30초 경계마다 커피 1초"),
    "phone": dict(f=1, fps=0, loop=False, note="사장님 메시지. 펄쩍 뒤 수화기 1초"),
}


def pose_geo(pose, dirn, f):
    g = dict(lift=0, hy=0, by=0, feet="stand", wl=("down", 0), wr=("down", 0), eyes="open", crouch=False)
    if pose == "idle" and dirn == "f" and f == 1:
        g["eyes"] = "closed"
    elif pose == "walk":
        if f == 1:
            g.update(hy=-1, by=-1, feet="stepL")
        elif f == 3:
            g.update(hy=-1, by=-1, feet="stepR")
    elif pose in ("sit", "type", "read", "sleep"):
        g.update(hy=1, by=1, feet="none")
        if pose == "type":
            g.update(wl=("fwd", -f), wr=("fwd", f - 1))
        elif pose == "read":
            g.update(wr=("fwd", -f))
        elif pose == "sleep":
            g.update(eyes="closed", hy=1 + f)
    elif pose == "jump":
        g.update(lift=3, wl=("up", 0), wr=("up", 0), feet="spread")
    elif pose == "scratch":
        g.update(wr=("head", f))
    elif pose == "panic":
        g.update(wl=("head", f), wr=("head", 1 - f), lift=f)
    elif pose == "cheer":
        g.update(lift=1, wl=("up", -1), wr=("up", -1))
    elif pose == "clap":
        st = "clapin" if f == 1 else "clapout"
        g.update(wl=(st, 0), wr=(st, 0))
    elif pose == "crouch":
        g.update(crouch=True, hy=2)
    elif pose == "carry" and dirn == "f":
        g.update(wl=("fwd", 0), wr=("fwd", 0))
    elif pose == "think":
        g.update(wr=("chin", f), hy=f)
    elif pose == "highfive":
        g.update(wr=("up", 0))
    elif pose == "stretch":
        g.update(wl=("up", f), wr=("up", f), eyes="closed", hy=-f)
    elif pose == "drink":
        g.update(wr=("fwd", -2 * f))
    elif pose == "phone":
        g.update(wr=("head", 3))
    return g


CROUCH_ROWS = [9, 10, 11, 12, 15, 16]


def torso_y(yt, g):
    if g["crouch"]:
        return 11 + CROUCH_ROWS.index(yt) if yt in CROUCH_ROWS else None
    return yt + g["by"]


def owl_body(sp, pose, dirn, f):
    """몸통 레이어 셀과 기하 정보. 셀 좌표는 템플릿 좌표(들어올림 전)."""
    S = SPECIES[sp]
    g = pose_geo(pose, dirn, f)
    cells = {}
    for r, row in enumerate(S["rows"]):
        yt = S["top"] + r
        for x, ch in enumerate(row):
            if ch == ".":
                continue
            if dirn == "b" and ch in "lLwiopF":
                ch = "b"
            cells[(x, yt + g["hy"])] = ch
    if g["eyes"] == "closed" and dirn == "f":
        for (x, y), ch in list(cells.items()):
            if ch in "ip":
                cells[(x, y)] = S["lid"]
        for x0, x1 in S["eyes"]:
            for x in range(x0, x1 + 1):
                cells[(x, S["lidrow"] + g["hy"])] = "k"
    rows = CROUCH_ROWS if g["crouch"] else list(range(9, 17))
    for yt in rows:
        row = TORSO[yt - 9]
        for x, ch in enumerate(row):
            if ch != ".":
                cells[(x, torso_y(yt, g))] = ch
    fy = 17
    feet = {"stand": [(3, fy), (4, fy), (9, fy), (10, fy)],
            "stepL": [(3, fy), (4, fy), (4, fy - 1), (9, fy - 1), (10, fy - 1)],
            "stepR": [(9, fy), (10, fy), (9, fy - 1), (3, fy - 1), (4, fy - 1)],
            "spread": [(2, fy), (3, fy), (10, fy), (11, fy)], "none": []}[g["feet"]]
    for p in feet:
        cells[p] = "y"
    hands = {}
    for side in ("l", "r"):
        st, dy = g["w" + side]
        mirror = (lambda x: 13 - x) if side == "r" else (lambda x: x)
        if st == "down" or g["crouch"]:
            hands[side] = (mirror(1), torso_y(12, g) + 1)
            continue
        for (x, yt), ch in WING_ERASED.items():
            y = torso_y(yt, g)
            if y is None:
                continue
            if ch is None:
                cells.pop((mirror(x), y), None)
            else:
                cells[(mirror(x), y)] = ch
        base = g["hy"] if st in HEAD_BLOBS else g["by"]
        pts = []
        for x, yt, ch in WING_BLOBS[st]:
            p = (mirror(x), yt + base + dy)
            cells[p] = ch
            pts.append(p)
        lowest = max(pts, key=lambda p: (p[1], -abs(p[0] - 6.5)))
        hands[side] = lowest
    if pose == "drink":
        y = 9 - 2 * f
        for x, dy, ch in [(10, 0, "k"), (11, 0, "M"), (12, 0, "k"), (10, 1, "m"), (11, 1, "m"),
                          (12, 1, "m"), (13, 1, "k"), (10, 2, "k"), (11, 2, "k"), (12, 2, "k")]:
            cells[(x, y + dy)] = ch
    elif pose == "phone":
        for x, y in [(11, 4), (12, 4), (10, 5), (11, 5), (10, 6), (10, 7), (10, 8), (11, 8), (11, 9), (12, 9)]:
            cells[(x, y)] = "k"
    g["hands"] = hands
    return cells, g


GLASSES_EYES = [(3, 4), (9, 10)]   # 모든 종의 눈 위치가 같다 (옷 레이어를 종과 무관하게 쓰기 위해)


def owl_outfit(outfit, g, dirn, chest, worn=False):
    """옷 레이어. outfit = '<rank>-<vendor>' | 'boss' | 'librarian'."""
    cells = {}
    if outfit == "boss":
        rank, uni, uni_d, tie = "boss", (92, 62, 140), (66, 42, 104), (244, 196, 48)
    elif outfit == "librarian":
        rank, uni, uni_d, tie = "librarian", (96, 136, 96), (70, 100, 70), None
    else:
        rank, vendor = outfit.split("-")
        th = THEMES[vendor]
        uni, uni_d, tie = th["uni"], th["uni_d"], (178, 58, 58)
    for p in chest:
        cells[p] = uni

    def T(x, yt, c):  # 몸통 기준 셀을 칠한다 (가슴이 있는 곳만)
        y = torso_y(yt, g)
        if y is not None and (x, y) in chest:
            cells[(x, y)] = c

    if dirn == "f":
        if rank in ("bujang", "boss"):
            for yt in ((11, 12, 13, 14) if worn and rank == "bujang" else (10, 11, 12, 13)):
                T(6, yt, tie)
                T(7, yt, tie)
        if rank == "gwajang":
            for yt in range(9, 15):
                for x in (2, 3, 4, 9, 10, 11):
                    T(x, yt, uni_d)
        if rank == "sawon":
            for x, yt, c in [(4, 9, (70, 110, 200)), (9, 9, (70, 110, 200)), (5, 10, (70, 110, 200)),
                             (8, 10, (70, 110, 200)), (6, 11, (250, 250, 250)), (7, 11, (250, 250, 250)),
                             (6, 12, (250, 250, 250)), (7, 12, (70, 110, 200))]:
                T(x, yt, c)
        if rank == "librarian":
            for yt in (10, 12, 14):
                T(6, yt, (240, 236, 220))
        if rank == "bujang":
            for x0, x1 in GLASSES_EYES:
                for y in (5, 6):
                    cells[(x0 - 1, y + g["hy"])] = (30, 30, 30)
                    cells[(x1 + 1, y + g["hy"])] = (30, 30, 30)
            (a0, a1), (b0, b1) = GLASSES_EYES
            for x in range(a1 + 2, b0 - 1):
                cells[(x, 5 + g["hy"])] = (30, 30, 30)
    else:
        if rank == "gwajang":
            for p in chest:
                cells[p] = uni_d
    # 소매: 날개를 내린 자세에서만
    for side, xs in (("l", 1), ("r", 12)):
        if g["w" + side][0] != "down" or g["crouch"]:
            continue
        sleeve = uni if rank == "gwajang" else uni_d
        for yt in (9, 10, 11):
            y = torso_y(yt, g)
            c = sleeve
            if (rank == "daeri" or worn) and yt == 10:
                c = (250, 250, 250)
            if (rank == "daeri" or worn) and yt == 11:
                continue
            cells[(xs, y)] = c
    if rank == "boss":
        crown = ["...Y..YY..Y...", "...YYYYYYYY...", "...YYYRRYYY..."]
        for r, row in enumerate(crown):
            for x, ch in enumerate(row):
                if ch != ".":
                    cells[(x, -1 + r + g["hy"])] = (244, 196, 48) if ch == "Y" else (210, 50, 60)
    return cells


def owl_wear(stage, g, dirn, chest):
    cells = {}
    for side, x in (("l", 1), ("r", 12)):
        if g["w" + side][0] == "down" and not g["crouch"]:
            cells[(x, torso_y(10, g))] = (250, 250, 250)
    if stage >= 2 and dirn == "f":
        for x0, x1 in GLASSES_EYES:
            for x in range(x0, x1 + 1):
                cells[(x, 7 + g["hy"])] = (106, 88, 98)
    if stage >= 3:
        for x, yt in ((3, 9), (4, 9), (3, 10), (4, 10), (3, 11), (4, 11), (3, 12)):
            y = torso_y(yt, g)
            if y is not None and (x, y) in chest:
                cells[(x, y)] = (214, 224, 220) if x == 3 else (248, 248, 232)
    return cells


def owl_role(role, g, dirn, chest):
    cells = {}

    def T(x, yt, c):
        y = torso_y(yt, g)
        if y is not None and (x, y) in chest:
            cells[(x, y)] = c

    if role == "backend":
        for x in range(2, 12):
            T(x, 9, (30, 30, 34))
    elif role == "devops":
        for x in range(2, 12):
            T(x, 14, (110, 74, 40))
        if dirn == "f":
            T(4, 15, (160, 160, 168))
            T(9, 15, (200, 64, 52))
    elif role == "writer":
        hy = g["hy"]
        for x, y, c in [(11, 3, (60, 60, 60)), (12, 2, (60, 90, 200)), (13, 1, (60, 90, 200))]:
            cells[(x, y + hy)] = c
    elif role == "security" and dirn == "f":
        for x in range(2, 12):
            cells[(x, 5 + g["hy"])] = (22, 24, 30)
            if x not in (6, 7):
                cells[(x, 6 + g["hy"])] = (22, 24, 30)
        cells[(3, 5 + g["hy"])] = (94, 104, 118)
        cells[(9, 5 + g["hy"])] = (94, 104, 118)
    elif role == "data" and dirn == "f":
        for x, yt in ((5, 9), (8, 9), (5, 10), (8, 10)):
            T(x, yt, (70, 112, 150))
        for yt in range(11, 14):
            for x in range(5, 9):
                T(x, yt, (242, 244, 232))
        for x, yt in ((5, 12), (6, 11), (6, 12), (7, 13), (8, 12), (8, 13)):
            T(x, yt, (56, 140, 152))
    elif role == "designer":
        for y, xs in ((-1, range(4, 10)), (0, range(3, 11)), (1, range(2, 12))):
            for x in xs:
                cells[(x, y + g["hy"])] = (74, 44, 94)
        cells[(8, -2 + g["hy"])] = (50, 32, 64)
        if dirn == "f" and g["wr"][0] == "down" and not g["crouch"]:
            for yt in range(10, 15):
                cells[(12, torso_y(yt, g))] = (152, 100, 54)
            cells[(12, torso_y(9, g))] = (224, 112, 80)
    elif role in ("frontend", "qa") and g["wl"][0] == "down" and not g["crouch"] and dirn == "f":
        y0 = torso_y(12, g)
        if role == "frontend":
            for i, c in enumerate([(232, 80, 70), (250, 200, 60), (70, 130, 230), (80, 180, 110)]):
                cells[(-1, y0 + i)] = c
                cells[(0, y0 + i)] = c
        else:
            ring = (90, 90, 96)
            for x, y in [(-1, -1), (0, -1), (1, -1), (-1, 0), (1, 0), (-1, 1), (0, 1), (1, 1)]:
                cells[(x, y0 + y)] = ring
            cells[(0, y0)] = (170, 220, 250)
            cells[(1, y0 + 2)] = (120, 80, 40)
            cells[(2, y0 + 3)] = (120, 80, 40)
    return cells


def owl_item(item, g, dirn, chest):
    cells = {}
    if item == "medal" and dirn == "f":
        for x, yt, c in [(5, 9, (66, 112, 190)), (8, 9, (190, 60, 64)),
                         (6, 10, (66, 112, 190)), (7, 10, (190, 60, 64)),
                         (6, 11, (246, 200, 60)), (7, 11, (246, 200, 60)),
                         (6, 12, (204, 142, 32)), (7, 12, (246, 200, 60))]:
            y = torso_y(yt, g)
            if y is not None and (x, y) in chest:
                cells[(x, y)] = c
    elif item == "blanket":
        for (x, y) in chest:
            cells[(x, y)] = (100, 140, 160) if (x + y) % 4 else (152, 184, 190)
        for side, x in (("l", 1), ("r", 12)):
            if g["w" + side][0] == "down" and not g["crouch"]:
                for yt in range(9, 14):
                    cells[(x, torso_y(yt, g))] = (100, 140, 160)
    elif item == "headphones":
        for x in range(2, 12):
            cells[(x, g["hy"])] = (44, 48, 60)
        for x in (1, 2, 11, 12):
            for y in range(2, 7):
                cells[(x, y + g["hy"])] = (44, 48, 60) if x in (1, 12) else (80, 104, 132)
    elif item == "helmet":
        for y, xs in ((-2, range(4, 10)), (-1, range(2, 12)), (0, range(1, 13)), (1, range(1, 13)), (2, range(0, 14))):
            for x in xs:
                cells[(x, y + g["hy"])] = (246, 194, 54) if y < 2 else (194, 138, 32)
        for y in range(-1, 2):
            cells[(7, y + g["hy"])] = (255, 224, 112)
    return cells


def cells_to_img(cells, lift, pal=None):
    img = Image.new("RGBA", (OWL_W, OWL_H), (0, 0, 0, 0))
    for (x, y), v in cells.items():
        c = pal[v] if pal is not None else v
        px, py = x + TX, y + TY - lift
        if 0 <= px < OWL_W and 0 <= py < OWL_H:
            img.putpixel((px, py), tuple(c) + (255,))
    return img


OUTFITS = [f"{r}-{v}" for r in ("bujang", "gwajang", "daeri", "sawon") for v in THEMES] + ["boss", "librarian"]
ROLES = ["backend", "frontend", "qa", "devops", "writer", "security", "data", "designer"]   # 수습(trainee)은 레이어 없음
RANK_KO = {"bujang": "부장", "gwajang": "과장", "daeri": "대리", "sawon": "사원", "boss": "사장님",
           "librarian": "사서"}
ROLE_KO = {"backend": "백엔드", "frontend": "프론트", "qa": "QA", "devops": "데브옵스", "writer": "테크라이터",
           "security": "보안", "data": "데이터", "designer": "디자이너", "trainee": "수습"}


def pose_frames():
    for pose, spec in POSES.items():
        for dirn in ("f", "b"):
            for f in range(spec.get(dirn, 0)):
                yield pose, dirn, f


def build_owls():
    """모든 종이 같은 뼈대(몸통 템플릿, 눈 위치)를 써서 옷·역할 레이어는 종과 무관하게 한 벌이다."""
    frames, anchors = {}, {}
    for pose, dirn, f in pose_frames():
        name = f"{pose}_{dirn}_{f}"
        for sp in SPECIES:
            pal = dict(BODY_PAL, **SPECIES[sp]["pal"])
            cells, g = owl_body(sp, pose, dirn, f)
            frames[f"body/{sp}/{name}"] = cells_to_img(cells, g["lift"], pal)
        cells, g = owl_body("eagle", pose, dirn, f)
        chest = {p for p, ch in cells.items() if ch == "c"}
        for of in OUTFITS:
            frames[f"outfit/{of}/{name}"] = cells_to_img(owl_outfit(of, g, dirn, chest), g["lift"])
            if of not in ("boss", "librarian"):
                # 투명한 근속 겹으로는 기존 소매·넥타이를 지울 수 없어 옷 생성기를 재사용한다.
                frames[f"outfit/{of}-worn/{name}"] = cells_to_img(owl_outfit(of, g, dirn, chest, worn=True), g["lift"])
        for stage in (1, 2, 3):
            frames[f"wear/{stage}/{name}"] = cells_to_img(owl_wear(stage, g, dirn, chest), g["lift"])
        for role in ROLES:
            frames[f"role/{role}/{name}"] = cells_to_img(owl_role(role, g, dirn, chest), g["lift"])
        for item in ("medal", "helmet", "blanket", "headphones"):
            frames[f"item/{item}/{name}"] = cells_to_img(owl_item(item, g, dirn, chest), g["lift"])
        hx, hy = g["hands"]["r"]
        anchors[name] = dict(hand=[hx + TX, hy + TY - g["lift"]], head=[OWL_AX, 2 + g["hy"] + TY - g["lift"]])
    return frames, anchors


# =====================================================================
# 사무실 소품
# =====================================================================
OFFICE = {}   # key -> dict(img, anchor)
PROPS = {}    # 소품 이름 -> 메타
WOOD = (164, 112, 70)


def reg(key, res):
    img, anchor = res
    OFFICE[key] = dict(img=img, anchor=[int(anchor[0]), int(anchor[1])])


def prop(name, kind, states, fp=(0, 0), h=0, fps=None, desc="", **extra):
    PROPS[name] = dict(kind=kind, footprint=[fp[0], fp[1]], height=h, states=states, desc=desc, **extra)
    if fps:
        PROPS[name]["fps"] = fps


def sprite_from_pix(rows, pal, ax, ay):
    return pix(rows, pal), (ax, ay)


def build_tiles():
    for name, col in FLOOR.items():
        a, b = (col, shade(col, 0.96))
        if name == "pantry":
            a, b = (222, 212, 194), col
        for v, c in (("a", a), ("b", b)):
            p = Painter(1, 1, 0, pad=0)
            p.poly([(0, 0, 0), (1, 0, 0), (1, 1, 0), (0, 1, 0)], c)
            reg(f"tile/{name}/{v}", p.result())
    prop("tile", "tile", ["a", "b"], (1, 1), 0, desc="바닥 타일. (i+j)%2==0 이면 a. 방 → 타일 이름은 floorplan.rooms[].floor",
         names=sorted(FLOOR))
    for name, (w, d, col) in {"doormat": (1.2, 1.6, (120, 96, 80)), "rug_review": (5.0, 1.9, shade(FLOOR["review"], 0.88))}.items():
        p = Painter(w, d, 0)
        p.poly([(0, 0, 0), (w, 0, 0), (w, d, 0), (0, d, 0)], col)
        reg(f"{name}/default", p.result())
        prop(name, "decal", ["default"], (w, d), 0, desc="바닥 깔개. 타일 위, 모든 물건 아래")


def build_shell():
    for t, th in THEMES.items():
        p = Painter(COLS, ROWS, WALL_H + 8, pad=8)
        p.poly([(0, 16, 0), (24, 16, 0), (24, 16, -5), (0, 16, -5)], (168, 160, 150))
        p.poly([(24, 0, 0), (24, 16, 0), (24, 16, -5), (24, 0, -5)], (140, 132, 124))
        p.poly([(0, 0, 0), (0, 16, 0), (0, 16, WALL_H), (0, 0, WALL_H)], shade(th["wall"], 0.9))
        p.poly([(0, 0, 0), (24, 0, 0), (24, 0, WALL_H), (0, 0, WALL_H)], th["wall"])
        for k in range(1, 48):
            g = k * 0.5
            if g < 16:
                p.line((0, g, 4), (0, g, WALL_H - 1), shade(th["stripe"], 0.9))
            if g < 24:
                p.line((g, 0, 4), (g, 0, WALL_H - 1), th["stripe"])
        p.poly([(0, 0, 0), (0, 16, 0), (0, 16, 4), (0, 0, 4)], shade(th["base"], 0.9))
        p.poly([(0, 0, 0), (24, 0, 0), (24, 0, 4), (0, 0, 4)], th["base"])
        p.poly([(-0.35, -0.35, WALL_H), (24, -0.35, WALL_H), (24, 0, WALL_H), (0, 0, WALL_H), (0, 16, WALL_H),
                (-0.35, 16, WALL_H)], shade(th["wall"], 1.08), shade(th["wall"], 0.6))
        p.poly([(-0.35, 16, 0), (0, 16, 0), (0, 16, WALL_H), (-0.35, 16, WALL_H)], shade(th["wall"], 0.7))
        p.poly([(24, -0.35, 0), (24, 0, 0), (24, 0, WALL_H), (24, -0.35, WALL_H)], shade(th["wall"], 0.62))
        reg(f"shell/{t}/default", p.result())
    prop("shell", "shell", list(THEMES), (COLS, ROWS), WALL_H, desc="벽 두 면과 바닥 판. 격자 (0,0)에 놓는다. 상태 이름 = 테마")


def wall_r(L, zmax, pad=2):
    return Painter(L, 0.3, zmax, pad=pad)


def wall_l(L, zmax, pad=2):
    return Painter(0.3, L, zmax, pad=pad)


def R(p, a, b, z1, z2, fill, outline=None):  # 오른쪽 뒷벽 면 (gy=0)
    p.poly([(a, 0, z1), (b, 0, z1), (b, 0, z2), (a, 0, z2)], fill, outline)


def Lw(p, a, b, z1, z2, fill, outline=None):  # 왼쪽 뒷벽 면 (gx=0)
    p.poly([(0, a, z1), (0, b, z1), (0, b, z2), (0, a, z2)], fill, outline)


def build_wall_items():
    # 창문
    for size, L in (("window_l", 2.7), ("window_xl", 3.4)):
        for st in ("day", "dusk", "night"):
            p = wall_r(L, 42)
            z1, z2 = 10, 40
            sky = {"day": (158, 208, 240), "dusk": (246, 178, 122), "night": (28, 34, 70)}[st]
            R(p, 0, L, z1, z2, sky)
            if st == "dusk":
                R(p, 0, L, 28, z2, (150, 112, 160))
            if st in ("night", "dusk"):
                if st == "night":
                    for u, v in [(0.2, 0.8), (0.5, 0.65), (0.75, 0.88), (0.35, 0.55), (0.9, 0.6), (0.1, 0.62)]:
                        p.dot((L * u, 0, z1 + (z2 - z1) * v), (255, 255, 230))
                    p.dot((L * 0.78, 0, z1 + (z2 - z1) * 0.78), (250, 236, 170), 3)
                n = int(L * 7)
                for k in range(n):
                    u = (k + 0.5) / n
                    hgt = 3 + (k * 7) % 7
                    x0, y0 = p.P(L * u, 0, z1)
                    p.d.line([(x0, y0 - 1), (x0, y0 - hgt)], fill=(16, 18, 38) if st == "night" else (90, 70, 96))
                    if k % 3 == 0 and st == "night":
                        p.d.point((x0, y0 - hgt + 2), fill=(255, 210, 110))
            else:
                for u, v, n in [(0.22, 0.75, 4), (0.6, 0.58, 3)]:
                    for k in range(n):
                        p.dot((L * u + k * 0.09, 0, z1 + (z2 - z1) * v), (255, 255, 255))
                        p.dot((L * u + k * 0.09 + 0.05, 0, z1 + (z2 - z1) * v + 1), (255, 255, 255))
            frame = (250, 250, 248)
            R(p, 0, L, z1, z2, None, frame)
            p.line((L / 2, 0, z1), (L / 2, 0, z2), frame)
            p.line((0, 0, (z1 + z2) / 2), (L, 0, (z1 + z2) / 2), frame)
            p.poly([(-0.1, 0, z1), (L + 0.1, 0, z1), (L + 0.1, 0.25, z1), (-0.1, 0.25, z1)], (246, 246, 244), (190, 190, 190))
            reg(f"{size}/{st}", p.result())
        prop(size, "window", ["day", "dusk", "night"], (L, 0), 40,
             desc="창문 (오른쪽 뒷벽). 06–17시 day, 17–20시 dusk, 20–06시 night. 조명 색을 곱하지 않는 레이어")
    # 칸반
    for size, L in (("kanban_l", 2.2), ("kanban_s", 1.8)):
        p = wall_r(L, 36)
        R(p, 0, L, 12, 36, (250, 250, 248), (150, 150, 150))
        for c in (1, 2):
            p.line((L * c / 3, 0, 13), (L * c / 3, 0, 33), (200, 200, 200))
        img, anc = p.result()
        reg(f"{size}/default", (img, anc))
        slots = []
        for col in range(3):
            for row in range(3):
                g0 = L * col / 3 + 0.12
                z0 = 31 - row * 6
                x, y = iso(g0, 0, z0 - 4)
                bx, by = iso(0, 0, 0)
                slots.append([int(round(x - bx)), int(round(y - by))])
        prop(size, "wall_r", ["default"], (L, 0), 36, desc="칸반 보드. 포스트잇은 wall_note 를 slots 에 놓는다 (열: 할 일·진행·완료)",
             slots=slots, slotOrder="열 우선: [할일0,할일1,할일2,진행0,…,완료2]", noteWidth=round(L / 3 - 0.24, 3))
    for c, col in {"yellow": (255, 214, 90), "pink": (255, 170, 160), "blue": (150, 210, 255), "green": (170, 230, 160)}.items():
        p = wall_r(0.36, 4, pad=1)
        R(p, 0, 0.36, 0, 4, col)
        reg(f"wall_note/{c}", p.result())
    prop("wall_note", "wall_r", ["yellow", "pink", "blue", "green"], (0.36, 0), 4, desc="칸반 포스트잇. 할 일 노랑·분홍, 진행 파랑, 완료 초록")
    # 사규 액자
    p = wall_r(1.0, 40)
    R(p, 0, 1.0, 12, 40, (232, 196, 110), (150, 110, 50))
    R(p, 0.12, 0.88, 15, 37, (252, 248, 236))
    for k in range(3):
        p.line((0.3 + k * 0.2, 0, 19), (0.3 + k * 0.2, 0, 34), (150, 140, 120))
    reg("rules_frame/default", p.result())
    prop("rules_frame", "wall_r", ["default"], (1.0, 0), 40, desc="사규 액자. 세션 제목을 세로로. 글자는 코드가 픽셀 글꼴로 그린다",
         textArea=dict(note="종이 영역 local u 0.12–0.88, z 15–37"))
    p = wall_r(0.45, 30)
    R(p, 0, 0.45, 22, 30, (90, 70, 50))
    R(p, 0.05, 0.4, 24, 28, (240, 230, 200))
    reg("joke_sign/default", p.result())
    prop("joke_sign", "wall_r", ["default"], (0.45, 0), 30, desc="고정 농담 팻말 (커피 리필은 셀프)")
    p = wall_r(0.5, 45)
    cx, cy = p.P(0.25, 0, 41)
    p.d.ellipse([cx - 3, cy - 3, cx + 3, cy + 3], fill=(250, 250, 250), outline=(80, 80, 80))
    p.d.line([(cx, cy), (cx, cy - 2)], fill=(40, 40, 40))
    p.d.line([(cx, cy), (cx + 2, cy)], fill=(40, 40, 40))
    reg("clock/default", p.result())
    prop("clock", "wall_r", ["default"], (0.5, 0), 45, desc="벽시계. 바늘은 정적, 경과 시간은 툴팁")
    for st in ("idle", "ring_0", "ring_1"):
        p = wall_r(0.5, 28)
        R(p, 0.15, 0.32, 22, 27, (230, 230, 226), (120, 120, 120))
        if st != "idle":
            p.dot((0.24, 0, 26), (230, 60, 50))
            off = 0 if st == "ring_0" else 1
            for s in (-1, 1):
                x, y = p.P(0.24, 0, 25)
                p.d.point((x + s * (4 + off), y - 1), fill=(60, 60, 60))
                p.d.point((x + s * (5 + off), y + 1), fill=(60, 60, 60))
        reg(f"interphone/{st}", p.result())
    prop("interphone", "wall_r", ["idle", "ring_0", "ring_1"], (0.5, 0), 28, fps=8, desc="인터폰. 사장님 등장 때 ring 1초")
    for st in ("off", "red", "yellow", "green"):
        p = wall_r(0.6, 38)
        R(p, 0, 0.6, 16, 38, (50, 54, 60), (30, 30, 30))
        for k, (name, on, off) in enumerate([("red", (240, 60, 50), (90, 40, 40)), ("yellow", (250, 200, 50), (90, 80, 40)),
                                             ("green", (70, 210, 100), (40, 80, 50))]):
            x, y = p.P(0.3, 0, 34 - k * 6)
            p.d.rectangle([x - 1, y - 1, x + 1, y + 1], fill=on if st == name else off)
        reg(f"ci_light/{st}", p.result())
    prop("ci_light", "wall_r", ["off", "red", "yellow", "green"], (0.6, 0), 38, desc="CI 신호등. 진행 중 yellow, 성공 green, 실패 red")
    for st in ("pass", "fail"):
        p = wall_r(1.8, 32)
        R(p, 0, 1.8, 16, 32, (252, 252, 250), (160, 160, 160))
        for k in range(4):
            p.line((0.2, 0, 29 - k * 3), (1.2, 0, 29 - k * 3), (170, 170, 170))
        c = (60, 170, 90) if st == "pass" else (220, 60, 50)
        x, y = p.P(1.4, 0, 21)
        p.d.rectangle([x - 2, y - 2, x + 2, y + 1], outline=c)
        reg(f"test_board/{st}", p.result())
    p = wall_r(1.8, 32)
    R(p, 0, 1.8, 16, 32, (34, 40, 48), (160, 160, 160))
    reg("test_board/counts", p.result())
    prop("test_board", "wall_r", ["pass", "fail", "counts"], (1.8, 0), 32, desc="오늘 확정 테스트 합격·실패 실행 누계. 숫자는 코드가 덧그린다")
    p = wall_r(0.75, 44)
    R(p, 0, 0.75, 37, 44, (252, 252, 246), (170, 170, 160))
    p.line((0.1, 0, 41), (0.6, 0, 41), (200, 60, 50))
    reg("job_ticket/default", p.result())
    prop("job_ticket", "wall_r", ["default"], (0.75, 0), 44, desc="작업표. 백그라운드 명령 1개당 1장, 랙 위 벽에 꽂힘")
    # 왼쪽 벽
    for st in ("off", "on"):
        p = wall_l(2.3, 34)
        Lw(p, 0, 2.3, 16, 34, (34, 38, 46), (20, 20, 24))
        if st == "on":
            for k in range(1, 4):
                p.line((0, 0.15, 18 + k * 4), (0, 2.15, 18 + k * 4), (44, 52, 62))
        reg(f"tv/{st}", p.result())
    bx, by = iso(0, 6.3, 0)
    quad = {k: [int(round(iso(0, gy, z)[0] - bx)), int(round(iso(0, gy, z)[1] - by))] for k, (gy, z) in
            {"bl": (6.45, 18), "br": (8.45, 18), "tl": (6.45, 32), "tr": (8.45, 32)}.items()}
    prop("tv", "wall_l", ["off", "on"], (0, 2.3), 34, desc="모니터링 TV (왼쪽 벽). 그래프 선은 코드가 screen 사각형 안에 그린다",
         screen=quad, lineColors=["#5ADC78", "#E6A05A", "#7AB4FF", "#E070A0"])
    p = wall_l(1.35, 30)
    Lw(p, 0, 1.35, 14, 30, (248, 248, 246), (130, 130, 130))
    for k in range(3):
        g = 1.35 * (k + 1) / 4
        p.line((0, g, 15), (0, g, 29), (210, 210, 210))
    reg("attendance/default", p.result())
    bx, by = iso(0, 11.1, 0)
    mslots = [[[int(round(iso(0, 11.1 + 1.35 * (c + 0.18) / 4, 27 - r * 3)[0] - bx)),
                int(round(iso(0, 11.1 + 1.35 * (c + 0.18) / 4, 27 - r * 3)[1] - by))] for r in range(5)] for c in range(4)]
    prop("attendance", "wall_l", ["default"], (0, 1.35), 30, desc="근태판. 칸: 출근·외근·외출·퇴근. 자석은 magnet 을 slots[칸][줄]에",
         slots=mslots, columns=["출근", "외근", "외출", "퇴근"])
    for c, col in {"red": (220, 70, 60), "blue": (60, 120, 220), "green": (60, 170, 90), "yellow": (240, 190, 40),
                   "gray": (150, 150, 150), "teal": TEAM_TINTS[0], "violet": TEAM_TINTS[1], "coral": TEAM_TINTS[2]}.items():
        img = Image.new("RGBA", (2, 2), tuple(col) + (255,))
        reg(f"magnet/{c}", (img, (0, 0)))
    prop("magnet", "fx", ["red", "blue", "green", "yellow", "gray", "teal", "violet", "coral"], desc="근태판 자석 2×2. 직원 한 명당 하나. 중소기업은 팀 색")
    for st in ("closed", "open"):
        p = wall_l(1.8, 32)
        Lw(p, 0, 1.8, 0, 32, (150, 104, 66), (90, 60, 40))
        if st == "closed":
            Lw(p, 0.15, 1.65, 2, 30, (168, 118, 76))
            p.dot((0, 1.35, 15), (240, 200, 80), 2)
        else:
            Lw(p, 0.15, 1.65, 0, 30, (40, 34, 30))
            Lw(p, 0.15, 0.55, 0, 30, (176, 124, 80))
        reg(f"door/{st}", p.result())
    prop("door", "wall_l", ["closed", "open"], (0, 1.8), 32, desc="출입문. 누가 드나드는 1초 동안 open")
    p = wall_l(1.2, 30)
    Lw(p, 0, 1.2, 14, 30, (40, 70, 56), (30, 40, 36))
    for k in range(4):
        p.line((0, 0.15, 27 - k * 3), (0, 0.95, 27 - k * 3), (220, 230, 220))
    reg("stats_board/default", p.result())
    prop("stats_board", "wall_l", ["default"], (0, 1.2), 30, desc="오늘의 통계 보드. 숫자는 코드가 그린다 (툴팁 권장)")


def build_furniture():
    # 서고
    p = Painter(0.8, 1.0, 28)
    p.box(0, 0, 0.8, 1.0, 28, WOOD)
    for v in (8, 15, 22):
        p.line((0.8, 0.1, v), (0.8, 0.9, v), shade(WOOD, 0.55))
    reg("cabinet_side/default", p.result())
    prop("cabinet_side", "floor", ["default"], (0.8, 1.0), 28, desc="서류 캐비닛 (왼쪽 벽 쪽, 서랍이 오른쪽 앞)")
    for st in ("closed", "open"):
        p = Painter(1.0, 1.3, 28)
        p.box(0, 0, 1.0, 0.8, 28, WOOD)
        for v in (8, 15, 22):
            p.line((0.1, 0.8, v), (0.9, 0.8, v), shade(WOOD, 0.55))
        for k, c in enumerate([(220, 80, 70), (80, 140, 220), (240, 200, 70)]):
            p.dot((0.25 + k * 0.22, 0.8, 25), c)
        if st == "open":
            p.box(0.1, 0.8, 0.8, 0.45, 6, shade(WOOD, 1.1), z=14)
            p.dot((0.45, 1.0, 21), (236, 204, 120), 2)
        reg(f"cabinet_front/{st}", p.result())
    prop("cabinet_front", "floor", ["closed", "open"], (1.0, 0.8), 28, desc="서류 캐비닛 (오른쪽 벽 쪽). 서류철 꺼낼 때 open")
    p = Painter(1.6, 0.9, 9)
    p.box(0, 0, 1.6, 0.9, 9, (196, 150, 100))
    reg("lib_desk/default", p.result())
    prop("lib_desk", "floor", ["default"], (1.6, 0.9), 9, desc="사서 책상")
    # 빌드실
    for st in ("led_0", "led_1"):
        p = Painter(1.1, 0.9, 36)
        p.box(0, 0, 1.1, 0.9, 36, (62, 66, 76))
        for row in range(6):
            for col in range(4):
                on = (row * 3 + col * 5 + (1 if st == "led_1" else 0) * (row + 1)) % 4 != 0
                c = (90, 230, 120) if on else (60, 90, 70)
                if (row + col) % 5 == 0:
                    c = (250, 190, 60) if st == "led_0" else (120, 90, 40)
                p.dot((0.15 + col * 0.24, 0.9, 8 + row * 4.5), c)
        reg(f"rack/{st}", p.result())
    prop("rack", "floor", ["led_0", "led_1"], (1.1, 0.9), 36, fps=3, desc="서버 랙. 셸 명령 중에만 깜빡임, 평소 led_0 고정")
    for st in ("off", "on"):
        p = Painter(0.8, 0.6, 16)
        p.box(0, 0, 0.8, 0.6, 16, (150, 156, 164))
        p.lq(0, 0, 0.8, 0.6, 0, 0.1, 0.7, 9, 14, (30, 40, 36))
        if st == "on":
            p.line((0.15, 0.6, 12), (0.5, 0.6, 12), (100, 230, 130))
            p.line((0.15, 0.6, 10), (0.4, 0.6, 10), (100, 230, 130))
        reg(f"terminal/{st}", p.result())
    prop("terminal", "floor", ["off", "on"], (0.8, 0.6), 16, desc="랙 옆 단말기")
    fan_pal = dict(k=(70, 74, 80), g=(170, 200, 220), b=(120, 150, 170))
    for st, rows in {"off": [".kkk.", "kgggk", "kgkgk", "kgggk", ".kkk.", "..k..", "..k..", ".kkk."],
                     "spin_0": [".kkk.", "kbgbk", "kgkgk", "kbgbk", ".kkk.", "..k..", "..k..", ".kkk."],
                     "spin_1": [".kkk.", "kgbgk", "kbkbk", "kgbgk", ".kkk.", "..k..", "..k..", ".kkk."]}.items():
        reg(f"fan/{st}", sprite_from_pix(rows, fan_pal, 2, 7))
    prop("fan", "floor", ["off", "spin_0", "spin_1"], (0.3, 0.3), 8, fps=10, desc="선풍기. 명령이 평균 2배 넘으면 돈다")
    # 테스트실
    arm = {"idle": 0, "run_0": 0, "run_1": 2}
    for st, dy in arm.items():
        p = Painter(2.6, 1.2, 22)
        p.box(0, 0, 2.6, 1.2, 12, (206, 210, 216))
        p.poly([(0.4, 0.3, 12.5), (1.8, 0.3, 12.5), (1.8, 0.8, 12.5), (0.4, 0.8, 12.5)], (60, 64, 70))
        armspr = pix(["....oo", "...k..", "..k...", ".k....", "oo....", "kk....", "kk...."],
                     dict(k=(120, 126, 134), o=(230, 130, 60)))
        x, y = p.P(1.5, 0.2, 12)
        p.paste(armspr, x - 1, y - 7 + dy)
        for k in range(4):
            c = (90, 230, 120) if (k < 3 or st == "idle") else (250, 190, 60)
            if st == "run_1" and k == 1:
                c = (250, 190, 60)
            p.dot((0.3 + k * 0.25, 1.2, 7), c)
        reg(f"test_machine/{st}", p.result())
    prop("test_machine", "floor", ["idle", "run_0", "run_1"], (2.6, 1.2), 22, fps=6, desc="자동 테스트 기계. 테스트 중 run 반복")
    p = Painter(1.0, 0.7, 9)
    p.box(0, 0, 1.0, 0.7, 9, (196, 150, 100))
    p.box(0.3, 0.2, 0.3, 0.25, 3, (60, 140, 80), z=9)
    reg("stamp_table/default", p.result())
    prop("stamp_table", "floor", ["default"], (1.0, 0.7), 9, desc="결과 도장 책상")
    # 팀장실
    bw = (124, 84, 54)
    p = Painter(3.2, 1.2, 10)
    p.box(0, 0, 3.2, 1.2, 10, bw)
    p.lq(0, 0, 3.2, 1.2, 0, 1.25, 1.95, 5, 8, (232, 196, 110))
    reg("boss_desk_l/default", p.result())
    prop("boss_desk_l", "floor", ["default"], (3.2, 1.2), 10, desc="대기업 부장 책상 (명패 포함)")
    p = Painter(1.7, 1.0, 9)
    p.box(0, 0, 1.7, 1.0, 9, bw)
    p.lq(0, 0, 1.7, 1.0, 0, 0.5, 1.2, 5, 8, (232, 196, 110))
    reg("boss_desk_s/default", p.result())
    prop("boss_desk_s", "floor", ["default"], (1.7, 1.0), 9, desc="중소기업 팀장석 책상 (명패 포함)")
    for name, (w, d, h, col) in {"chair_boss": (0.6, 0.5, 14, (50, 50, 56)), "chair": (0.6, 0.45, 12, (60, 60, 66))}.items():
        p = Painter(w, d, h)
        p.box(0, 0, w, d, h, col)
        reg(f"{name}/default", p.result())
        prop(name, "floor", ["default"], (w, d), h, desc="의자")
    for name, (w, h) in {"monitor_lg": (1.0, 10), "monitor_md": (0.8, 10), "monitor_sm": (0.6, 9)}.items():
        states = ["off", "code_0", "code_1", "read_0", "read_1", "saver_0", "saver_1"]
        for st in states:
            p = Painter(w, 0.12, h)
            p.box(0, 0, w, 0.12, h, (44, 46, 52))
            base = {"off": (32, 34, 40), "code": (22, 40, 30), "read": (34, 46, 64), "saver": (40, 60, 110)}[st.split("_")[0]]
            p.lq(0, 0, w, 0.12, 0, 0.06, w - 0.06, 2, h - 1, base)
            kind = st.split("_")[0]
            k2 = int(st[-1]) if st != "off" else 0
            if kind in ("code", "read"):
                lc = (100, 230, 130) if kind == "code" else (170, 200, 240)
                lens = [0.8, 0.5, 0.95, 0.6] if k2 == 0 else [0.5, 0.95, 0.6, 0.8]
                for k in range(3):
                    v = h - 3 - k * 2
                    if v < 3:
                        break
                    p.line((0.1, 0.12, v), (0.1 + (w - 0.25) * lens[k], 0.12, v), lc)
            elif kind == "saver":
                p.dot((w * (0.3 if k2 == 0 else 0.6), 0.12, h - 4 - k2), (250, 220, 120), 2)
            reg(f"{name}/{st}", p.result())
        prop(name, "floor", states, (w, 0.12), h, fps=2,
             desc="모니터. 수정 code, 열람 read, 결재 대기 saver('요구사항 대기 중'), 자리 비움 off. 책상 위 z = 책상 높이")
    p = Painter(2.6, 1.0, 9)
    p.box(0, 0, 2.6, 1.0, 9, (226, 218, 202))
    reg("dev_desk/default", p.result())
    prop("dev_desk", "floor", ["default"], (2.6, 1.0), 9, desc="개발실 파티션 책상")
    p = Painter(2.6, 0.12, 18)
    p.box(0, 0, 2.6, 0.12, 18, (255, 255, 255))
    reg("partition/default", p.result())
    prop("partition", "floor", ["default"], (2.6, 0.12), 18,
         desc="개발실 파티션. 흰색 원본에 tint 를 곱한다. 대기업은 테마 cubicle 색, 중소기업은 팀 색")
    p = Painter(1.4, 0.7, 8)
    p.box(0, 0, 1.4, 0.7, 8, (200, 190, 170))
    reg("folding_desk/default", p.result())
    prop("folding_desk", "floor", ["default"], (1.4, 0.7), 8, desc="임시 책상(복도 4자리)과 간이 책상(리뷰실 2자리)")
    p = Painter(1.2, 0.8, 12)
    p.box(0, 0, 1.2, 0.8, 12, (220, 222, 224))
    p.poly([(0.15, 0.1, 12.5), (0.9, 0.1, 12.5), (0.9, 0.5, 12.5), (0.15, 0.5, 12.5)], (70, 74, 80))
    reg("copier/default", p.result())
    prop("copier", "floor", ["default"], (1.2, 0.8), 12, desc="복사기")
    for st, (size, wilted) in {"s0": (0.15, False), "s1": (0.35, False), "s2": (0.55, False), "s3": (0.75, False),
                               "s4": (1.0, False), "wilted": (1.0, True)}.items():
        img = plant_sprite(size, wilted)
        reg(f"plant/{st}", (img, (5, img.height - 1)))
    prop("plant", "floor", ["s0", "s1", "s2", "s3", "s4", "wilted"], (0.5, 0.5), 19,
         desc="화분. 컨텍스트 사용률 0–20 s0, –40 s1, –60 s2, –80 s3, –100 s4, 넘치면 wilted. 요약(compaction)이면 s0 새 화분")
    for st, n in (("empty", 0), ("some", 2), ("full", 4)):
        p = Painter(0.5, 0.5, 9)
        p.box(0, 0, 0.5, 0.5, 7, (150, 156, 162))
        for k in range(n):
            p.dot((0.12 + k * 0.08, 0.15 + (k % 2) * 0.15, 8), (250, 250, 248), 2)
        reg(f"trash/{st}", p.result())
    prop("trash", "floor", ["empty", "some", "full"], (0.5, 0.5), 9,
         desc="휴지통. 사고 0 empty, 1–4 some, 5+ full. 10개 넘으면 paper_ball 을 바닥 slots 에 굴린다")
    img = Image.new("RGBA", (3, 3), (0, 0, 0, 0))
    ImageDraw.Draw(img).rectangle([0, 0, 2, 2], fill=(250, 250, 248), outline=(200, 200, 196))
    reg("paper_ball/default", (img, (0, 2)))
    prop("paper_ball", "fx", ["default"], (0, 0), 3, desc="구겨진 종이. 휴지통 앞 바닥",
         slots=[[round(14.6 - k * 0.35, 2), round(10.75 + (k % 2) * 0.2, 2)] for k in range(10)])
    # 탕비실
    p = Painter(2.6, 0.8, 12)
    p.box(0, 0, 2.6, 0.8, 12, (240, 238, 232))
    reg("counter/default", p.result())
    prop("counter", "floor", ["default"], (2.6, 0.8), 12, desc="탕비실 조리대")
    for st in ("idle", "brew_0", "brew_1"):
        p = Painter(0.7, 0.55, 20)
        p.box(0, 0, 0.7, 0.55, 11, (54, 54, 58))
        p.dot((0.5, 0.55, 8), (230, 60, 50) if st != "idle" else (90, 50, 50))
        if st != "idle":
            x, y = p.P(0.35, 0.3, 12)
            off = 0 if st == "brew_0" else 1
            for k in range(3):
                p.d.point((x + ((k + off) % 2), y - 2 - k * 2), fill=(255, 255, 255, 170))
        reg(f"coffee_machine/{st}", p.result())
    prop("coffee_machine", "floor", ["idle", "brew_0", "brew_1"], (0.7, 0.55), 20, fps=4, desc="커피머신. 조리대 위 z=12")
    p = Painter(1.0, 0.8, 30)
    p.box(0, 0, 1.0, 0.8, 30, (214, 220, 226))
    p.lq(0, 0, 1.0, 0.8, 0, 0.12, 0.88, 4, 27, (170, 210, 230))
    for row in range(4):
        for col in range(4):
            c = [(60, 200, 120), (60, 120, 230), (240, 90, 80), (250, 200, 60)][(row + col) % 4]
            p.dot((0.22 + col * 0.19, 0.8, 7 + row * 5.5), c)
    reg("fridge/default", p.result())
    prop("fridge", "floor", ["default"], (1.0, 0.8), 30, desc="에너지 드링크 냉장고")
    p = Painter(1.8, 0.6, 8)
    p.box(0, 0, 1.8, 0.6, 8, (90, 70, 56))
    reg("tv_stand/default", p.result())
    prop("tv_stand", "floor", ["default"], (1.8, 0.6), 8, desc="게임기 받침")
    for st in ("off", "game_0", "game_1"):
        p = Painter(1.6, 0.18, 15)
        p.box(0, 0, 1.6, 0.18, 15, (30, 30, 34))
        p.lq(0, 0, 1.6, 0.18, 0, 0.1, 1.5, 2, 13, (60, 140, 220) if st != "off" else (34, 36, 42))
        if st != "off":
            o = 0 if st == "game_0" else 1
            for k, c in enumerate([(250, 220, 80), (240, 90, 90), (120, 230, 120)]):
                p.dot((0.4 + k * 0.35, 0.18, 5 + ((k + o) % 3) * 2), c, 2)
        reg(f"game_tv/{st}", p.result())
    prop("game_tv", "floor", ["off", "game_0", "game_1"], (1.6, 0.18), 15, fps=3, desc="콘솔 게임기 화면. 받침 위 z=8")
    for t, th in THEMES.items():
        s = th["sofa"]
        p = Painter(3.35, 1.1, 14)
        p.box(0.1, 0, 3.0, 0.3, 14, s)
        p.box(0, 0, 0.25, 1.1, 9, s)
        p.box(0.1, 0.3, 3.0, 0.8, 6, shade(s, 1.12))
        p.box(3.1, 0, 0.25, 1.1, 9, s)
        reg(f"sofa_{t}/default", p.result())
        prop(f"sofa_{t}", "floor", ["default"], (3.35, 1.1), 14, desc="탕비실 소파 (등받이가 뒤). 앉은 직원은 소파보다 깊이 +0.2, z=4")
        p = Painter(3.6, 0.8, 6)
        p.box(0, 0, 3.6, 0.8, 6, shade(s, 1.1))
        reg(f"review_sofa_seat_{t}/default", p.result())
        p = Painter(3.6, 0.3, 12)
        p.box(0, 0, 3.6, 0.3, 12, s)
        reg(f"review_sofa_back_{t}/default", p.result())
        prop(f"review_sofa_seat_{t}", "floor", ["default"], (3.6, 0.8), 6, desc="리뷰실 소파 방석. 앉은 직원은 방석과 등받이 사이 깊이")
        prop(f"review_sofa_back_{t}", "floor", ["default"], (3.6, 0.3), 12, desc="리뷰실 소파 등받이 (보는 사람 쪽)")
    p = Painter(1.6, 0.6, 5)
    p.box(0, 0, 1.6, 0.6, 5, (200, 170, 120))
    reg("coffee_table/default", p.result())
    prop("coffee_table", "floor", ["default"], (1.6, 0.6), 5, desc="탕비실 낮은 탁자")
    p = Painter(1.2, 0.45, 2)
    p.box(0, 0, 1.2, 0.45, 2, (214, 168, 106))
    p.dot((0.5, 0.2, 2), (210, 60, 50))
    reg("pizza_box/default", p.result())
    prop("pizza_box", "floor", ["default"], (1.2, 0.45), 2,
         desc="피자 상자. 활성 2시간부터 1시간마다 1개. 탁자 위 z=5, 상자마다 z+2", stack=dict(gx=8.45, gy=15.17, z0=5, dz=2, max=8))
    reg("ramen/default", (pix(["wwww", "rwwr", "rrrr", ".rr."], dict(w=(250, 250, 250), r=(210, 60, 50))), (2, 3)))
    prop("ramen", "floor", ["default"], (0.2, 0.2), 4, desc="컵라면. 야근 모드에만. 조리대 위 z=12")
    p = Painter(1.0, 1.2, 18)
    p.box(0, 0, 1.0, 0.3, 18, (80, 56, 46))
    p.box(0, 0.3, 1.0, 0.9, 8, (100, 72, 58))
    reg("massage_chair/default", p.result())
    prop("massage_chair", "floor", ["default"], (1.0, 1.2), 18, desc="안마의자. 누운 직원 z=6")
    # 출입문 · 출고장
    p = Painter(1.2, 0.7, 14)
    p.box(0, 0, 1.2, 0.7, 9, (200, 170, 130))
    p.box(0.2, 0.1, 0.6, 0.45, 5, (60, 62, 70), z=9)
    reg("label_table/default", p.result())
    prop("label_table", "floor", ["default"], (1.2, 0.7), 14, desc="라벨 프린터 탁자")
    for name, (w, d, h, col) in {"box_lg": (0.8, 0.7, 9, (196, 150, 95)), "box_md": (0.7, 0.6, 8, (206, 160, 104)),
                                 "box_sm": (0.55, 0.5, 7, (186, 140, 88))}.items():
        p = Painter(w, d, h)
        p.box(0, 0, w, d, h, col)
        reg(f"{name}/default", p.result())
        prop(name, "floor", ["default"], (w, d), h, desc="택배 상자")
    p = Painter(0.7, 0.6, 8)
    p.box(0, 0, 0.7, 0.6, 8, (206, 160, 104))
    p.line((0.35, 0, 8), (0.35, 0.6, 8), (230, 214, 170))
    p.lq(0, 0, 0.7, 0.6, 0, 0.15, 0.45, 2, 5, (250, 250, 248))
    reg("box_taped/default", p.result())
    prop("box_taped", "floor", ["default"], (0.7, 0.6), 8, desc="포장된 상자 (커밋). 라벨에 커밋 메시지 첫 줄은 툴팁")
    # 리뷰실
    for st in ("off", "code", "approved"):
        p = Painter(3.2, 0.25, 24)
        p.box(0, 0, 3.2, 0.25, 24, (40, 42, 48))
        p.lq(0, 0, 3.2, 0.25, 0, 0.15, 3.05, 6, 22, (30, 44, 62) if st != "off" else (34, 36, 42))
        if st != "off":
            for k in range(4):
                p.line((0.3, 0.25, 19 - k * 3), (0.3 + [2.0, 1.4, 2.4, 1.0][k], 0.25, 19 - k * 3), (140, 190, 240))
        if st == "approved":
            x, y = p.P(2.6, 0.25, 10)
            p.d.rectangle([x - 3, y - 2, x + 3, y + 2], outline=(70, 200, 100))
        reg(f"review_screen/{st}", p.result())
    bx, by = iso(0, 0, 0)
    pslots = [[int(round(iso(0.4 + k * 0.42, 0.25, 8 + (k % 2) * 6)[0] - bx)),
               int(round(iso(0.4 + k * 0.42, 0.25, 8 + (k % 2) * 6)[1] - by))] for k in range(6)]
    prop("review_screen", "floor", ["off", "code", "approved"], (3.2, 0.25), 24,
         desc="리뷰실 큰 화면. 리뷰 중 code, 승인 문구 approved. 지적마다 postit_red 를 slots 에 (이 소품 위치 기준 px)", slots=pslots)
    p = Painter(0.3, 0.05, 5, pad=1)
    p.poly([(0, 0, 0), (0.3, 0, 0), (0.3, 0, 4), (0, 0, 4)], (240, 70, 70))
    reg("postit_red/default", p.result())
    prop("postit_red", "fx", ["default"], (0.3, 0), 4, desc="리뷰 지적 포스트잇")
    p = Painter(0.3, 0.3, 9)
    p.box(0, 0, 0.3, 0.3, 6, (220, 60, 60))
    p.dot((0.12, 0.12, 8), (40, 40, 40))
    p.dot((0.2, 0.15, 9), (220, 60, 60))
    reg("pen_cup/default", p.result())
    prop("pen_cup", "floor", ["default"], (0.3, 0.3), 9, desc="빨간펜 통")
    # 장애 상황실
    for st, col in (("off", (110, 110, 108)), ("on", (250, 250, 248)), ("scribble", (250, 250, 248))):
        p = Painter(2.6, 0.2, 24)
        p.box(0, 0, 2.6, 0.2, 24, col)
        if st == "scribble":
            for k in range(4):
                p.line((0.3, 0.2, 19 - k * 4), (0.3 + [1.8, 1.2, 2.0, 0.9][k], 0.2, 19 - k * 4 + (k % 2)),
                       [(220, 60, 60), (60, 90, 200), (220, 60, 60), (40, 40, 40)][k])
        reg(f"whiteboard/{st}", p.result())
    prop("whiteboard", "floor", ["off", "on", "scribble"], (2.6, 0.2), 24, desc="화이트보드. 평소 off(어두움), 장애 scribble")
    bpal = dict(r=(120, 60, 60), k=(40, 40, 40))
    reg("beacon/off", sprite_from_pix([".rrr.", "rrrrr", "rrrrr", "kkkkk"], bpal, 2, 3))
    reg("beacon/on_0", sprite_from_pix([".rRr.", "rRRRr", "rRRRr", "kkkkk"], dict(r=(220, 40, 40), R=(255, 140, 120), k=(40, 40, 40)), 2, 3))
    reg("beacon/on_1", sprite_from_pix([".Rrr.", "RRrrr", "Rrrrr", "kkkkk"], dict(r=(220, 40, 40), R=(255, 140, 120), k=(40, 40, 40)), 2, 3))
    prop("beacon", "floor", ["off", "on_0", "on_1"], (0.3, 0.1), 4, fps=6, desc="경광등. 화이트보드 위 z=24")
    for st, col in (("off", (90, 80, 70)), ("on", (150, 120, 90))):
        p = Painter(2.2, 1.0, 8)
        p.box(0, 0, 2.2, 1.0, 8, col)
        reg(f"incident_table/{st}", p.result())
    prop("incident_table", "floor", ["off", "on"], (2.2, 1.0), 8, desc="상황실 탁자")
    p = Painter(2.0, 0.15, 16)
    p.box(0, 0, 2.0, 0.15, 9, (24, 24, 26), z=7)
    reg("safety_board/default", p.result())
    ox_, oy_ = iso(0.3, 0.15, 7 + 7)
    bx, by = iso(0, 0, 0)
    prop("safety_board", "floor", ["default"], (2.0, 0.15), 16, desc="무재해 전광판 (분). 숫자는 digit 를 digitOrigin 부터 digitStep 씩",
         digitOrigin=[int(round(ox_ - bx)), int(round(oy_ - by))], digitStep=[4, 2], maxDigits=4)
    digits = {"0": ["###", "#.#", "#.#", "#.#", "###"], "1": [".#.", "##.", ".#.", ".#.", "###"],
              "2": ["###", "..#", "###", "#..", "###"], "3": ["###", "..#", "###", "..#", "###"],
              "4": ["#.#", "#.#", "###", "..#", "..#"], "5": ["###", "#..", "###", "..#", "###"],
              "6": ["###", "#..", "###", "#.#", "###"], "7": ["###", "..#", ".#.", ".#.", ".#."],
              "8": ["###", "#.#", "###", "#.#", "###"], "9": ["###", "#.#", "###", "..#", "###"]}
    for k, rows in digits.items():
        img = Image.new("RGBA", (3, 6), (0, 0, 0, 0))
        for y, row in enumerate(rows):
            for x, ch in enumerate(row):
                if ch == "#":
                    img.putpixel((x, y + x // 2), (255, 80, 60, 255))
        reg(f"digit/{k}", (img, (0, 0)))
    prop("digit", "fx", list(digits), (0, 0), 6, desc="전광판 숫자 (면 기울기에 맞춰 기울임)")
    # 거래처 미팅실
    for st in ("off", "on"):
        p = Painter(2.6, 0.2, 20)
        p.box(0, 0, 2.6, 0.2, 20, (40, 42, 48))
        p.lq(0, 0, 2.6, 0.2, 0, 0.12, 2.48, 6, 18, (36, 40, 50) if st == "off" else (52, 74, 120))
        if st == "on":
            p.lq(0, 0, 2.6, 0.2, 0, 0.6, 2.0, 10, 14, (230, 236, 248))
        reg(f"meeting_screen/{st}", p.result())
    prop("meeting_screen", "floor", ["off", "on"], (2.6, 0.2), 20, desc="화상회의 화면. MCP 미팅 중 on, 밝은 띠에 거래처 상호를 코드가 쓴다")
    p = Painter(2.8, 1.3, 9)
    p.box(0, 0, 2.8, 1.3, 9, (210, 184, 140))
    reg("meeting_table/default", p.result())
    prop("meeting_table", "floor", ["default"], (2.8, 1.3), 9, desc="긴 회의 탁자")
    for st, n in (("empty", 0), ("s1", 4), ("s2", 8), ("s3", 12), ("default", 12)):
        p = Painter(0.6, 1.4, 22)
        p.box(0, 0, 0.6, 1.4, 22, (150, 104, 66))
        for i in range(n):
            k, row = i % 6, i // 6
            c = [(220, 80, 70), (80, 140, 220), (240, 200, 70), (80, 170, 110), (180, 100, 200), (240, 140, 60)][k]
            x, y = p.P(0.6, 0.15 + k * 0.2, 6 + row * 8)
            p.d.line([(x, y), (x, y - 4)], fill=c)
        reg(f"bookshelf/{st}", p.result())
    prop("bookshelf", "floor", ["default", "empty", "s1", "s2", "s3"], (0.6, 1.4), 22,
         desc="사무실 모든 직원의 고유 읽기 수 합계: 0–24 empty, 25–49 s1, 50–99 s2, 100 이상 s3")

    # 새 방도 기존 투영과 상자 도구만 쓴다.
    p = Painter(2.4, 1.0, 9)
    p.box(0, 0, 2.4, 1.0, 9, (182, 160, 126))
    reg("conference_table/default", p.result())
    prop("conference_table", "floor", ["default"], (2.4, 1.0), 9, desc="계획 회의 탁자")
    p = Painter(2.6, 0.2, 24)
    p.box(0, 0, 2.6, 0.2, 24, (246, 246, 238))
    for i, width in enumerate((1.8, 1.2, 1.5)):
        p.line((0.3, 0.2, 18-i*5), (0.3+width, 0.2, 18-i*5), (62, 110, 148))
    reg("conference_whiteboard/default", p.result())
    prop("conference_whiteboard", "floor", ["default"], (2.6, 0.2), 24, desc="계획 회의 화이트보드")
    for state in ("idle", "shred"):
        p = Painter(0.9, 0.8, 15)
        p.box(0, 0, 0.9, 0.8, 11, (78, 86, 90))
        p.box(0.12, 0.2, 0.65, 0.12, 1, (28, 32, 34), z=11)
        if state == "shred":
            p.box(0.2, 0.22, 0.5, 0.03, 4, (246, 242, 220), z=11)
            for i in range(4):
                p.line((0.2+i*0.14, 0.82, 8), (0.2+i*0.14, 0.82, 3), (246, 242, 220))
        reg(f"shredder/{state}", p.result())
    prop("shredder", "floor", ["idle", "shred"], (0.9, 0.8), 15, desc="새 압축 방문 동안 종이를 파쇄")
    p = Painter(1.8, 0.6, 17)
    p.box(0, 0, 1.8, 0.6, 10, (116, 134, 126))
    p.box(0, 0, 1.8, 0.1, 7, (192, 214, 218), z=10)
    reg("security_window/default", p.result())
    prop("security_window", "floor", ["default"], (1.8, 0.6), 17, desc="경비실 결재 창구")
    p = Painter(0.25, 1.0, 12)
    p.box(0, 0, 0.25, 0.25, 12, (94, 106, 106))
    p.box(0, 0, 0.15, 1.0, 2, (230, 188, 64), z=9)
    reg("security_barrier/default", p.result())
    prop("security_barrier", "floor", ["default"], (0.25, 1.0), 12, desc="경비실 차단봉")
    p = Painter(2.2, 0.7, 10)
    p.box(0, 0.1, 2.2, 0.5, 4, (148, 114, 78))
    p.box(0, 0, 2.2, 0.12, 10, (132, 102, 74))
    reg("roof_bench/default", p.result())
    prop("roof_bench", "floor", ["default"], (2.2, 0.7), 10, desc="옥상 휴식 벤치")
    p = Painter(1.1, 0.9, 25)
    p.box(0, 0, 1.1, 0.9, 25, (80, 122, 150))
    p.box(0.12, 0.9, 0.62, 0.02, 12, (174, 206, 216), z=9)
    p.box(0.8, 0.9, 0.14, 0.02, 3, (244, 212, 100), z=14)
    reg("roof_vending/default", p.result())
    prop("roof_vending", "floor", ["default"], (1.1, 0.9), 25, desc="옥상 음료 자판기")

    for name in ("boss_desk_l", "boss_desk_s", "dev_desk", "folding_desk"):
        w, d = PROPS[name]["footprint"]
        PROPS[name]["progressSlots"] = dict(coffee=[w - 0.32, d - 0.28], papers=[0.1, d - 0.36], lamp=[w - 0.3, 0.02])


def plant_sprite(size=0.6, wilted=False):
    leaf = (86, 156, 74) if not wilted else (176, 156, 84)
    leaf_d = (60, 120, 56) if not wilted else (136, 116, 64)
    h = 4 + int(size * 10)
    img = Image.new("RGBA", (11, h + 5), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    for i in range(h):
        wv = 2 + int((i / max(h - 1, 1)) * 3)
        if wilted:
            wv = 1 + int((i / max(h - 1, 1)) * 5)
        d.line([(5 - wv // 2 - (1 if i % 3 == 0 else 0), i), (5 + wv // 2, i)], fill=leaf if i % 2 else leaf_d)
    d.rectangle([2, h, 8, h + 4], fill=(170, 96, 60))
    d.line([(1, h), (9, h)], fill=(196, 120, 80))
    d.line([(3, h + 4), (7, h + 4)], fill=(120, 66, 40))
    return img


def build_fx():
    """손에 드는 물건, 연출 효과, 오버레이."""
    held = {
        "duck": (["..yy.", ".yyyo", "yyyy.", ".yyy."], dict(y=(250, 214, 60), o=(232, 120, 40)), (1, 2)),
        "folder": (["kkkkk", "kmmmk", "kmmmk", "kmmmk", "kmmmk", "kkkkk"], dict(k=(120, 90, 40), m=(236, 204, 120)), (1, 1)),
        "clipboard": ([".kk.", "kwwk", "kwwk", "kwwk", "kkkk"], dict(k=(120, 84, 50), w=(250, 250, 246)), (1, 1)),
        "envelope": (["kkkkk", "kwkwk", "kwwwk", "kkkkk"], dict(k=(150, 120, 80), w=(236, 214, 170)), (1, 1)),
        "box_carry": (["kkkkk", "kbbbk", "kbbbk", "kkkkk"], dict(k=(140, 100, 60), b=(206, 160, 104)), (1, 1)),
        "papers": (["wwww.", "wkkww", "wwwww", ".wwww"], dict(w=(250, 250, 248), k=(150, 150, 150)), (1, 1)),
        "briefcase": ([".kkk.", "kkkkk", "kbbbk", "kbbbk", "kkkkk"], dict(k=(78, 58, 46), b=(132, 92, 56)), (1, 1)),
        "flashlight": (["...yy", "kkkyy", ".kk.."], dict(k=(60, 64, 76), y=(255, 222, 114)), (1, 1)),
        "scroll": (["kkkkkk", "kwwwwk", ".wwww.", ".wkkw.", ".wwww.", "kkkkkk"], dict(k=(152, 108, 54), w=(248, 230, 180)), (1, 1)),
        "swatch": (["rrr", "yyy", "bbb"], dict(r=(232, 80, 70), y=(250, 200, 60), b=(70, 130, 230)), (1, 1)),
    }
    for name, (rows, pal, anc) in held.items():
        reg(f"held_{name}/default", sprite_from_pix(rows, pal, *anc))
        prop(f"held_{name}", "held", ["default"], desc="손에 드는 물건. 기준점을 올빼미 hand 기준점에 맞춘다")
    mpal = dict(s=(255, 255, 255, 150), k=(60, 60, 60), w=(240, 240, 240))
    reg("mug/still", sprite_from_pix(["....", "....", "kkk.", "kwkk", "kwk.", "kkk."], mpal, 1, 5))
    reg("mug/steam_0", sprite_from_pix([".s.s", "s.s.", "kkk.", "kwkk", "kwk.", "kkk."], mpal, 1, 5))
    reg("mug/steam_1", sprite_from_pix(["s.s.", ".s.s", "kkk.", "kwkk", "kwk.", "kkk."], mpal, 1, 5))
    prop("mug", "floor", ["still", "steam_0", "steam_1"], (0.2, 0.2), 6, fps=3, desc="부장 머그컵. 응답 대기 중 김이 오른다. 책상 위 z=책상 높이")
    reg("duck_desk/default", sprite_from_pix(["..yy.", ".yyyo", "yyyy.", ".yyy."], dict(y=(250, 214, 60), o=(232, 120, 40)), 2, 3))
    prop("duck_desk", "floor", ["default"], (0.2, 0.2), 4, desc="책상 위 러버덕. 부장이 집어 들면 숨기고 held_duck")
    for n in range(1, 5):
        p = Painter(0.3, 0.3, 3 + n * 2)
        for k in range(n):
            p.box(0, 0, 0.22, 0.22, 3, (250, 246, 232), z=k * 2)
            p.line((0, 0.22, k * 2 + 1), (0.22, 0.22, k * 2 + 1), (176, 121, 77))
        reg(f"coffee_stack/s{n}", p.result())
        p = Painter(0.45, 0.35, n * 2)
        for k in range(n):
            p.box(0, 0, 0.45, 0.35, 1, (250, 250, 244), z=k * 2)
            p.line((0.1, 0.05, k * 2 + 1), (0.35, 0.05, k * 2 + 1), (150, 160, 172))
        reg(f"paper_stack/s{n}", p.result())
    prop("coffee_stack", "floor", [f"s{n}" for n in range(1, 5)], (0.3, 0.3), 11,
         desc="입사 1시간마다 종이컵 한 잔, 최대 4잔. 책상 위")
    prop("paper_stack", "floor", [f"s{n}" for n in range(1, 5)], (0.45, 0.35), 8,
         desc="고유 수정 파일 10개마다 서류 한 단. 책상 위 최대 4단")
    for st in ("off", "on"):
        p = Painter(0.3, 0.3, 11)
        p.box(0, 0, 0.3, 0.3, 1, (55, 65, 76))
        p.line((0.15, 0.15, 1), (0.15, 0.15, 9), (65, 75, 86))
        p.box(0, 0, 0.3, 0.3, 2, (255, 215, 110) if st == "on" else (90, 100, 110), z=9)
        reg(f"desk_lamp/{st}", p.result())
    prop("desk_lamp", "floor", ["off", "on"], (0.3, 0.3), 11, desc="직원 책상 야근 램프")
    glow = Image.new("RGBA", (41, 23), (0, 0, 0, 0))
    gd = ImageDraw.Draw(glow)
    for inset, alpha in [(0, 12), (4, 20), (8, 28)]:
        gd.ellipse((inset, inset // 2, 40 - inset, 22 - inset // 2), fill=(255, 217, 128, alpha))
    reg("glow_lamp/default", (glow, (20, 14)))
    prop("glow_lamp", "overlay", ["default"], desc="실제 야간 조명에서 활성 직원 책상 램프의 비네트")
    # 효과
    reg("sweat/0", sprite_from_pix([".b", "bb", "bb"], dict(b=(120, 190, 250)), 0, 2))
    reg("sweat/1", sprite_from_pix(["..", ".b", "bb"], dict(b=(120, 190, 250)), 0, 2))
    prop("sweat", "fx", ["0", "1"], fps=4, desc="땀방울. 머리 기준점 오른쪽 위 (+6, -2). 2분 넘는 명령")
    fw = {0: ["...y...", ".......", "y..r..y", ".......", "...y..."],
          1: ["y..r..y", ".r...r.", "r.yyy.r", ".r...r.", "y..r..y"],
          2: ["r.....r", ".......", "...y...", ".......", "r.....r"]}
    for k, rows in fw.items():
        img = pix(rows, dict(y=(250, 214, 60), r=(240, 90, 90)))
        reg(f"fireworks/{k}", (img.resize((14, 10), Image.NEAREST), (7, 10)))
    prop("fireworks", "fx", ["0", "1", "2"], fps=6, desc="릴리스 폭죽. 출고장 위, 0→1→2 한 번")
    for st, k, cab in (("drive_0", 0, (220, 70, 60)), ("drive_1", 1, (220, 70, 60)), ("pr", 0, (55, 125, 220))):
        p = Painter(1.2, 3.1, 18)
        p.box(0, 0, 1.2, 2.2, 16, (240, 240, 236))
        p.box(0, 2.2, 1.2, 0.9, 12, cab)
        p.lq(0, 2.2, 1.2, 0.9, 0, 0.15, 1.05, 6, 10, (150, 200, 230))
        for gy in (0.6, 2.6):
            x, y = p.P(1.2, gy, 0)
            p.d.rectangle([x - 1, y - 2, x + 1, y], fill=(30, 30, 30))
            p.d.point((x - 1 + k * 2, y - 1), fill=(160, 160, 160))
        reg(f"truck/{st}", p.result())
    prop("truck", "fx", ["drive_0", "drive_1", "pr"], (1.2, 3.1), 18, fps=8,
         desc="push 성공 때 출발하는 트럭. PR 연결이 있으면 파란 pr 트럭 주차")
    p = Painter(0.5, 0.1, 9)
    p.box(0, 0, 0.5, 0.1, 8, (250, 248, 240))
    p.lq(0, 0, 0.5, 0.1, 0, 0.08, 0.42, 3, 6, (220, 80, 70))
    reg("sign_away/default", p.result())
    prop("sign_away", "floor", ["default"], (0.5, 0.1), 9, desc="'외근 중' 팻말. 자리 책상 위. 도메인 이름은 툴팁")
    lad = ["k...k", "kkkkk", "k...k", "k...k", "kkkkk", "k...k", "k...k", "kkkkk", "k...k", "k...k", "kkkkk", "k...k",
           "k...k", "kkkkk", "k...k", "k...k", "kkkkk", "k...k", "k...k", "kkkkk", "k...k"]
    reg("ladder/default", sprite_from_pix(lad, dict(k=(170, 120, 70)), 2, len(lad) - 1))
    prop("ladder", "fx", ["default"], desc="사다리. 세션 제목이 바뀌면 사규 액자 아래 4초")
    for name, rows, pal in [("note_temp", ["wwwwww", "wkkkkw", "wwwwww", "wkkkww", "wwwwww"], dict(w=(255, 248, 200), k=(120, 110, 80))),
                            ("note_full", ["wwwwww", "wrrrrw", "wwwwww", "wrrrww", "wwwwww"], dict(w=(255, 240, 240), r=(210, 60, 60))),
                            ("note_return", ["wwwwww", "wrrrrw", "wwwwww", "wrrrrw", "wwwwww"], dict(w=(255, 255, 255), r=(210, 60, 60)))]:
        reg(f"{name}/default", sprite_from_pix(rows, pal, 0, 4))
        prop(name, "fx", ["default"], desc={"note_temp": "'임시 책상' 쪽지", "note_full": "'만석' 쪽지", "note_return": "'반송' 쪽지 (push 실패 상자)"}[name])
    for name, col, dashed in [("stamp_pass", (40, 160, 80), False), ("stamp_fail", (210, 50, 50), False),
                              ("stamp_pass_est", (40, 160, 80), True), ("stamp_fail_est", (210, 50, 50), True),
                              ("stamp_reject", (210, 50, 50), False)]:
        img = Image.new("RGBA", (9, 7), (0, 0, 0, 0))
        d = ImageDraw.Draw(img)
        for x in range(9):
            for y in (0, 6):
                if not dashed or x % 2 == 0:
                    img.putpixel((x, y), col + (255,))
        for y in range(7):
            for x in (0, 8):
                if not dashed or y % 2 == 0:
                    img.putpixel((x, y), col + (255,))
        if name == "stamp_reject":
            d.line([(2, 2), (6, 4)], fill=col)
            d.line([(2, 4), (6, 2)], fill=col)
        else:
            d.line([(2, 3), (6, 3)], fill=col)
        reg(f"{name}/default", (img, (4, 6)))
        prop(name, "fx", ["default"], desc={"stamp_pass": "테스트 합격 도장", "stamp_fail": "테스트 반려 도장",
                                             "stamp_pass_est": "합격 도장 (추정: 파이프 섞인 명령)", "stamp_fail_est": "반려 도장 (추정)",
                                             "stamp_reject": "사장님 반려 도장 (결재판 위)"}[name])
    reg("manual_open/default", sprite_from_pix(["kkkk.kkkk", "kwwwkwwwk", "kwkwkwkwk", "kwwwkwwwk", "kkkkkkkkk"],
                                               dict(k=(90, 60, 40), w=(250, 246, 230)), 4, 4))
    prop("manual_open", "fx", ["default"], desc="펼친 외주 매뉴얼 (스킬 사용). 회의 탁자 위 z=9. 스킬 이름은 툴팁")
    reg("business_card/default", sprite_from_pix(["kkkk", "kwwk", "kkkk"], dict(k=(120, 120, 120), w=(250, 250, 250)), 2, 2))
    prop("business_card", "fx", ["default"], desc="명함 (MCP 미팅). 회의 탁자 위 z=9")
    cloud = ["..kkk.....", ".kwwwk.kk.", "kwwwwwkwwk", "kwwwwwwwwk", ".kkkkkkkk."]
    reg("cloud_lag/default", sprite_from_pix(cloud, dict(k=(150, 150, 160), w=(240, 240, 244)), 5, 4))
    prop("cloud_lag", "fx", ["default"], desc="'통신 지연' 구름. 사무실 위 왼쪽 위 모서리 (WSL 응답 느림)")
    # 장애 상황실 빛 오버레이
    p = Painter(8.0, 3.0, 30)
    glow = Image.new("RGBA", p.img.size, (0, 0, 0, 0))
    gd = ImageDraw.Draw(glow)
    gd.polygon([p.P(0, 0, 0), p.P(8, 0, 0), p.P(8, 3, 0), p.P(0, 3, 0)], fill=(255, 40, 30, 40))
    bx, by = p.P(3.5, 0.3, 26)
    gd.polygon([(bx, by), (bx - 30, by - 9), (bx - 30, by + 1)], fill=(255, 60, 40, 70))
    gd.polygon([(bx, by), (bx + 30, by - 1), (bx + 30, by + 9)], fill=(255, 60, 40, 70))
    bb = glow.getbbox()
    reg("glow_incident/default", (glow.crop(bb), (p.ax - bb[0], p.ay - bb[1])))
    prop("glow_incident", "overlay", ["default"], (8, 3), 30, desc="장애 상황실 경광등 빛. 격자 (16, 9)에 두고 모든 물건 위, 야간 색 곱하기 뒤")


# =====================================================================
# 평면도
# =====================================================================
PARTITIONS = [(3.9, 0, 0.2, 3.6, 16), (5.9, 0, 0.2, 3.6, 16), (10.9, 0, 0.2, 3.6, 16), (15.9, 0, 0.2, 4.0, 16),
              (0, 4.9, 1.6, 0.2, 6), (3.2, 4.9, 1.0, 0.2, 6), (5.7, 4.9, 2.0, 0.2, 6), (9.2, 4.9, 3.4, 0.2, 6), (14.2, 4.9, 1.7, 0.2, 6),
              (16, 5.9, 1.4, 0.2, 6), (19.4, 5.9, 4.6, 0.2, 6), (15.9, 6.0, 0.2, 1.2, 6), (15.9, 8.6, 0.2, 2.4, 6),
              (16, 8.9, 5, 0.2, 6), (22.4, 8.9, 1.6, 0.2, 6), (16, 11.9, 1.2, 0.2, 6), (18.8, 11.9, 2.4, 0.2, 6), (22.8, 11.9, 1.2, 0.2, 6), (19.9, 12.0, 0.2, 4.0, 6),
              (15.9, 12.2, 0.2, 1.6, 6), (15.9, 15.0, 0.2, 1.0, 6), (0, 10.9, 1.0, 0.2, 6), (3, 10.9, 3.5, 0.2, 6),
              (9, 10.9, 4.5, 0.2, 6), (4.9, 11.0, 0.2, 0.4, 6), (4.9, 12.6, 0.2, 3.4, 6), (0, 12.9, 1.2, 0.2, 6), (2.8, 12.9, 2.2, 0.2, 6), (12.9, 11.0, 0.2, 0.4, 6), (12.9, 12.6, 0.2, 3.4, 6)]


def build_wallsegs():
    for n, (gx, gy, w, d, h) in enumerate(PARTITIONS, 1):
        p = Painter(w, d, h)
        p.box(0, 0, w, d, h, (232, 228, 220))
        reg(f"wallseg_{n:02d}/default", p.result())
        prop(f"wallseg_{n:02d}", "floor", ["default"], (w, d), h, desc="방 사이 낮은 칸막이 (고정 구조물)")


DEV_DESKS = [(1.0, 6.4), (4.4, 6.4), (7.8, 6.4), (1.0, 9.0), (4.4, 9.0), (7.8, 9.0)]
TEMP_DESKS = [(11.3, 6.6), (13.6, 6.6), (11.3, 8.8), (13.6, 8.8)]
BOSS_SMALL = [(16.5 + i * 1.9, 2.4) for i in range(4)]
SPARE_SMALL = [(22.1, 6.6), (22.1, 7.8)]


def build_layout(kind):
    smb = kind == "small"
    L = []

    def put(pid, sprite, gx, gy, state="default", z=0, depth=None, layer="world", **kw):
        fp = PROPS[sprite]["footprint"]
        if depth is None:
            depth = gx + fp[0] / 2 + gy + fp[1] / 2
        x, y = rnd(iso(gx, gy, z))
        L.append(dict(id=pid, sprite=sprite, state=state, gx=gx, gy=gy, z=z, depth=round(depth, 3), x=x, y=y,
                      layer=layer, **kw))

    # 벽 소품
    put("window", "window_xl" if smb else "window_l", 18.6 if smb else 19.2, 0, "day", layer="window",
        drive="Rust daylight: 06–17 day, 17–20 dusk, 20–06 night")
    put("kanban", "kanban_s" if smb else "kanban_l", 16.5 if smb else 16.6, 0, layer="wall", drive="하이라이트 팀(없으면 최근 팀)의 할 일 목록")
    put("rules_frame", "rules_frame", 22.3 if smb else 22.2, 0, layer="wall", drive="세션 제목")
    put("joke_sign", "joke_sign", 23.45, 0, layer="wall")
    put("clock", "clock", 16.0, 0, layer="wall")
    put("interphone", "interphone", 18.2 if smb else 18.8, 0, "idle", layer="wall", drive="사장님 등장 때 ring 1초")
    put("ci_light", "ci_light", 10.0, 0, "off", layer="wall", drive="SharedProps.ci: building→yellow, ok→green, fail→red")
    put("test_board", "test_board", 13.4, 0, "pass", layer="wall", drive="마지막 테스트 결과")
    put("job_ticket", "job_ticket", 7.85, 0, layer="wall", visible=False, drive="백그라운드 명령이 있을 때만")
    put("tv", "tv", 0, 6.3, "on", layer="wall", drive="팀별 컨텍스트 사용량 선")
    put("attendance", "attendance", 0, 11.1, layer="wall", drive="직원마다 자석 하나")
    put("door", "door", 0, 13.6, "closed", layer="wall", drive="누가 드나드는 1초 동안 open")
    put("stats_board", "stats_board", 0, 14.65, layer="wall")
    put("doormat", "doormat", 0, 13.6, layer="decal")
    put("rug_review", "rug_review", 17.6, 6.7, layer="decal")
    # 칸막이
    for n, (gx, gy, w, d, h) in enumerate(PARTITIONS, 1):
        put(f"wallseg_{n:02d}", f"wallseg_{n:02d}", gx, gy)
    # 서고
    for k, gy in enumerate((0.4, 1.5, 2.6)):
        put(f"cabinet_side_{k}", "cabinet_side", 0.15, gy)
    for k, gx in enumerate((0.8, 1.85, 2.9)):
        put(f"cabinet_front_{k}", "cabinet_front", gx, 0.15, "closed", drive="서류철 꺼낼 때 그 직원과 가장 가까운 것 open" if k == 1 else None)
    put("lib_desk", "lib_desk", 2.2, 3.0, depth=6.5)
    # 빌드실
    put("rack_0", "rack", 6.4, 0.3, "led_0", drive="셸 명령 중 led_0/led_1 반복")
    put("rack_1", "rack", 7.7, 0.3, "led_0")
    put("terminal", "terminal", 9.4, 2.6, "off", depth=12.7)
    put("fan", "fan", 6.9, 3.9, "off", depth=10.9, drive="명령이 평균 2배 넘으면 spin")
    # 테스트실
    put("test_machine", "test_machine", 11.8, 0.6, "idle", depth=13.0, drive="테스트 중 run_0/run_1")
    put("stamp_table", "stamp_table", 14.6, 2.9, depth=17.0)
    # 팀장실
    if smb:
        for i, (gx, gy) in enumerate(BOSS_SMALL):
            D = gx + 0.85 + gy + 0.5
            put(f"boss_chair_{i}", "chair_boss", gx + 0.8, gy - 0.6, depth=D - 0.5)
            put(f"boss_desk_{i}", "boss_desk_s", gx, gy, depth=D)
            put(f"boss_mon_{i}_0", "monitor_md", gx + 0.15, gy + 0.15, "off", z=9, depth=D + 0.01)
            put(f"plant_boss_{i}", "plant", gx + 0.8, 0.6, "s0", visible=False, drive="이 자리 부장의 컨텍스트 사용률")
    else:
        gx, gy = 18.6, 2.6
        D = gx + 1.6 + gy + 0.6
        put("boss_chair_0", "chair_boss", gx + 2.3, gy - 0.6, depth=D - 0.5)
        put("boss_desk_0", "boss_desk_l", gx, gy, depth=D)
        put("boss_mon_0_0", "monitor_lg", gx + 0.25, gy + 0.15, "off", z=10, depth=D + 0.01)
        put("boss_mon_0_1", "monitor_lg", gx + 1.3, gy + 0.15, "off", z=10, depth=D + 0.01)
        put("mug_0", "mug", gx + 2.7, gy + 0.8, "still", z=10, depth=D + 0.04, drive="응답 대기 중 steam")
        put("duck_0", "duck_desk", gx + 2.3, gy + 0.9, z=10, depth=D + 0.04, drive="부장이 집어 들면 숨김")
        put("plant_boss_0", "plant", 22.7, 0.7, "s0", drive="부장 컨텍스트 사용률")
    # 개발실
    for i, (gx, gy) in enumerate(DEV_DESKS):
        D = gx + 1.3 + gy + 0.5
        put(f"dev_part_{i}", "partition", gx, gy - 0.55, depth=D - 0.8, tint=None, drive="대기업: 테마 cubicle 색. 중소기업: 앉은 직원의 팀 색")
        put(f"dev_chair_{i}", "chair", gx + 1.5, gy - 0.4, depth=D - 0.5)
        put(f"dev_desk_{i}", "dev_desk", gx, gy, depth=D)
        put(f"dev_mon_{i}_0", "monitor_sm", gx + 0.15, gy + 0.15, "off", z=9, depth=D + 0.01)
        put(f"dev_mon_{i}_1", "monitor_sm", gx + 0.8, gy + 0.15, "off", z=9, depth=D + 0.01)
    for i, (gx, gy) in enumerate(TEMP_DESKS):
        put(f"temp_desk_{i}", "folding_desk", gx, gy, visible=False, drive="서브 7명째부터 펼친다")
    put("note_temp", "note_temp", 11.5, 6.5, z=8, visible=False, layer="fx")
    put("trash", "trash", 14.9, 10.1, "empty", depth=25.6, drive="사고 수")
    put("copier", "copier", 13.4, 5.4, depth=19.4)
    put("plant_dev", "plant", 11.6, 5.6, "s3", depth=17.2)
    # 탕비실
    put("counter", "counter", 5.3, 11.3, depth=17.3)
    put("coffee", "coffee_machine", 5.5, 11.4, "idle", z=12, depth=17.31)
    put("ramen", "ramen", 7.0, 11.6, z=12, depth=17.32, visible=False, drive="야근 모드")
    put("fridge", "fridge", 8.2, 11.25, depth=20.0)
    put("tv_stand", "tv_stand", 9.6, 11.25, depth=21.9)
    put("game_tv", "game_tv", 9.7, 11.3, "off", z=8, depth=21.91, drive="게임기 앞에 직원이 있으면 game")
    put("sofa", "sofa_claude", 7.6, 13.5, depth=22.8, themed=True)
    put("coffee_table", "coffee_table", 8.3, 15.1, depth=24.6)
    put("massage_chair", "massage_chair", 11.7, 13.3, depth=25.6)
    # 출입문 · 출고장
    put("label_table", "label_table", 3.4, 13.1, depth=17.5)
    put("box_0", "box_lg", 2.2, 14.4, depth=17.3)
    put("box_1", "box_md", 3.1, 14.6, depth=18.1)
    put("box_2", "box_sm", 2.35, 14.5, z=9, depth=17.31)
    # 리뷰실
    put("review_screen", "review_screen", 18.2, 6.15, "off", depth=26.0)
    put("review_seat", "review_sofa_seat_claude", 18.0, 7.6, depth=27.8, themed=True)
    put("review_back", "review_sofa_back_claude", 18.0, 8.3, depth=28.3, themed=True)
    put("pen_cup", "pen_cup", 21.8, 6.5, depth=28.45)
    if smb:
        for i, (gx, gy) in enumerate(SPARE_SMALL):
            put(f"spare_desk_{i}", "folding_desk", gx, gy, visible=False, drive="중소기업 5팀째부터 간이 책상")
    # 장애 상황실
    put("whiteboard", "whiteboard", 18.2, 9.25, "off", depth=28.85)
    put("beacon", "beacon", 19.5, 9.3, "off", z=24, depth=28.87)
    put("incident_table", "incident_table", 18.4, 10.3, "off", depth=30.2)
    put("safety_board", "safety_board", 16.3, 8.6, depth=25.9, drive="무재해 분: 팀 중 최솟값")
    # 미팅실
    put("meeting_screen", "meeting_screen", 20.6, 12.15, "off", depth=34.0)
    put("meeting_table", "meeting_table", 20.6, 13.5, depth=36.15)
    put("bookshelf", "bookshelf", 23.1, 14.3, depth=38.4)
    put("plant_meeting", "plant", 23.4, 15.4, "s4", depth=38.8)
    put("conference_whiteboard", "conference_whiteboard", 16.4, 12.3)
    put("conference_table", "conference_table", 16.7, 13.5)
    put("shredder", "shredder", 4.3, 0.7, "idle")
    put("security_window", "security_window", 1.0, 11.15)
    put("security_barrier", "security_barrier", 4.1, 11.3)
    put("roof_bench", "roof_bench", 13.2, 13.2, depth=28.0)
    put("roof_vending", "roof_vending", 14.6, 11.2)
    # 오버레이
    put("glow_incident", "glow_incident", 16.0, 9.0, layer="overlay", visible=False, drive="장애 판정 중")
    return L


def seats(kind):
    smb = kind == "small"
    s = {"dev": [], "temp": [], "boss": [], "spare": []}
    for i, (gx, gy) in enumerate(DEV_DESKS):
        D = gx + 1.3 + gy + 0.5
        s["dev"].append(dict(id=f"dev_{i}", gx=round(gx + 1.75, 2), gy=round(gy - 0.15, 2), depth=round(D - 0.3, 3),
                             desk=f"dev_desk_{i}", monitors=[f"dev_mon_{i}_0", f"dev_mon_{i}_1"], partition=f"dev_part_{i}"))
    for i, (gx, gy) in enumerate(TEMP_DESKS):
        D = gx + 0.7 + gy + 0.35
        s["temp"].append(dict(id=f"temp_{i}", gx=round(gx + 0.9, 2), gy=round(gy - 0.15, 2), depth=round(D - 0.3, 3),
                              desk=f"temp_desk_{i}"))
    if smb:
        for i, (gx, gy) in enumerate(BOSS_SMALL):
            D = gx + 0.85 + gy + 0.5
            s["boss"].append(dict(id=f"boss_{i}", gx=round(gx + 1.2, 2), gy=round(gy - 0.2, 2), depth=round(D - 0.3, 3),
                                  desk=f"boss_desk_{i}", monitors=[f"boss_mon_{i}_0"], plant=f"plant_boss_{i}"))
        for i, (gx, gy) in enumerate(SPARE_SMALL):
            D = gx + 0.7 + gy + 0.35
            s["spare"].append(dict(id=f"spare_{i}", gx=round(gx + 0.8, 2), gy=round(gy - 0.15, 2), depth=round(D - 0.3, 3),
                                   desk=f"spare_desk_{i}"))
    else:
        s["boss"].append(dict(id="boss_0", gx=20.9, gy=2.15, depth=23.0, desk="boss_desk_0",
                              monitors=["boss_mon_0_0", "boss_mon_0_1"], plant="plant_boss_0", mug="mug_0", duck="duck_0"))
    return s


STANDS = {
    "library": [[1.4, 1.7], [1.2, 3.5], [3.0, 2.0]], "librarian": [[2.8, 2.95]],
    "build_terminal": [[9.8, 3.7]], "build": [[8.2, 2.4], [7.0, 2.4]],
    "test": [[13.3, 2.75], [12.4, 2.8], [15.0, 3.9]],
    "review_sofa": [[18.8, 7.95], [19.8, 7.95], [20.8, 7.95]],
    "incident": [[19.6, 9.95], [21.8, 10.8], [22.8, 10.3], [17.4, 10.7], [18.2, 11.6], [21.0, 11.6]],
    "meeting": [[20.9, 13.2], [22.4, 13.2], [20.9, 15.1], [22.4, 15.1], [23.6, 14.2]],
    "pantry_console": [[10.5, 12.6]], "pantry_massage": [[12.2, 13.9, 6]], "pantry_sofa": [[8.6, 14.1, 4], [9.7, 14.1, 4]],
    "pantry": [[5.9, 12.6], [6.4, 14.4]], "door": [[0.9, 14.1]], "door_wait": [[1.7, 13.6]], "shipping": [[2.9, 14.0], [3.8, 14.0]],
    "boss_window": [[20.3, 1.3]], "boss_pace": [[18.9, 4.4], [21.6, 4.4]], "dev_center": [[6.6, 8.0]], "exit": [[0.2, 14.1]],
    "conference": [[17.2, 13.2], [18.7, 13.2], [17.2, 15.1], [18.7, 15.1]],
    "archive": [[5, 2], [5, 3.6]], "security": [[2.1, 11.8], [3.3, 11.8]],
    "roof": [[13.8, 13.8, 4], [14.9, 13.8, 4], [14.7, 15.1]],
}
ENTRIES = {"library": [2, 5], "archive": [5, 5], "build": [8.45, 5], "test": [13.4, 5], "boss": [16, 4.9], "review": [16, 7.9],
           "incident": [21.7, 9], "conference": [18, 12], "meeting": [22, 12], "pantry": [7.75, 11], "roof": [13, 12],
           "security": [5, 12], "entrance": [2, 13], "dev": [13.5, 8]}
LABELS = {"library": [177, 74], "archive": [228, 106], "build": [271, 138], "test": [337, 170], "boss": [405, 203], "dev": [250, 192], "review": [382, 235], "incident": [340, 267], "conference": [244, 267], "meeting": [294, 301], "security": [106, 140], "entrance": [51, 171], "pantry": [136, 201], "roof": [200, 233]}


def floorplan_json():
    rooms = {}
    for k, (name, a, b, c, e, floor) in ROOMS.items():
        lx, ly = LABELS[k]
        rooms[k] = dict(name=name, rect=[a, b, c, e], floor=floor, label=[lx, ly], entry=ENTRIES[k])
    return dict(
        version=1,
        canvas1x=dict(w=W, h=H), origin=dict(x=OX, y=OY), tile=dict(w=TILE_W, h=TILE_H), grid=dict(cols=COLS, rows=ROWS),
        wallHeight=WALL_H,
        iso="x = origin.x + (gx - gy) * 12, y = origin.y + (gx + gy) * 6 - z (1x 픽셀). 소품의 x,y 는 이 값을 반올림한 것",
        depth="깊이 = gx + w/2 + gy + d/2 (발자국 중심). 배치마다 depth 를 따로 적어 둔 것이 우선. 같은 깊이면 배열 순서",
        floorRule="타일 (i,j)는 rooms 의 rect 로 방을 찾는다. 방 floor 가 'boss' 면 'boss_<테마>', 'incident' 면 장애 중 'incident_on' 아니면 'incident_off'. (i+j)%2==0 이면 /a 아니면 /b",
        rooms=rooms,
        themes={t: dict(label=v["label"], cubicle=hexc(v["cubicle"]), uniform=hexc(v["uni"]), sofa=hexc(v["sofa"]),
                        accent=hexc(v["accent"])) for t, v in THEMES.items()},
        teamTints=[hexc(c) for c in TEAM_TINTS],
        nightTint=hexc(NIGHT_TINT),
        lightTints=dict(evening=hexc(EVENING_TINT), night=hexc(NIGHT_TINT), dim="#B4B4BE", off="#6E6E82",
                        rule="곱하기 색. evening: 17–20시, night: 20–06시. off > dim > 시간표. dim: 30분 조용하지만 프로세스 살아 있음(S58). off: 퇴근(S36). "
                             "창문 레이어에는 곱하지 않는다. 바뀔 때 1.5초에 걸쳐 보간"),
        fx=dict(cloud_lag=[8, 8], note="cloud_lag 는 캔버스 1x 좌표 (8,8)에 기준점. sweat 는 머리 기준점 + (6,-2)"),
        layouts={k: dict(placements=build_layout(k), seats=seats(k)) for k in ("large", "small")},
        stands=STANDS,
        path="이동: 현재 위치 → 현재 방 entry → (방이 다르면) dev entry → 목표 방 entry → 목표. 직선 구간, 0.5–1.5초",
        cast=dict(owlAnchor=[OWL_AX, OWL_AY], frame=[OWL_W, OWL_H],
                  dir="걸어가는 방향의 화면 y가 커지면(아래로) f, 작아지면 b. 화면 x가 작아지면(왼쪽) scale.x = -1"),
    )


# =====================================================================
# 참조 렌더 (앱 렌더러가 따라야 할 순서)
# =====================================================================
def place(canvas, key, x, y, flip=False, tint=None):
    e = OFFICE.get(key) or OWLS.get(key)
    img = e["img"]
    ax, ay = e["anchor"]
    if tint is not None:
        img = tinted(img, tint)
    if flip:
        img = ImageOps.mirror(img)
        ax = img.width - ax
    canvas.alpha_composite(img, (int(x - ax), int(y - ay)))


def owl_keys(c):
    name = f"{c['pose']}_{c.get('dir', 'f')}_{c.get('frame', 0)}"
    wear = c.get("wear", 0) if c["outfit"] not in ("boss", "librarian") else 0
    outfit = c["outfit"] + ("-worn" if wear else "")
    keys = [f"body/{c['species']}/{name}", f"outfit/{outfit}/{name}"]
    if wear:
        keys.append(f"wear/{wear}/{name}")
    if c.get("role", "trainee") != "trainee":
        keys.append(f"role/{c['role']}/{name}")
    items = c.get("items", [])
    for item in ("medal", "blanket", "headphones", "helmet"):
        if item not in items or (item == "medal" and "blanket" in items) or (item == "headphones" and "helmet" in items):
            continue
        keys.append(f"item/{item}/{name}")
    return keys, name


def draw_owl(canvas, c):
    x, y = rnd(iso(c["gx"], c["gy"], c.get("z", 0)))
    keys, name = owl_keys(c)
    flip = c.get("flip", False)
    for k in keys:
        if k in OWLS:   # 소품이 안 보이는 자세는 role 프레임이 없다
            place(canvas, k, x, y, flip)
    if c.get("held"):
        hx, hy = OWL_META["anchors"][name]["hand"]
        if flip:
            hx = OWL_W - hx
        place(canvas, f"held_{c['held']}/default", x - OWL_AX + hx, y - OWL_AY + hy, flip)
    return x, y


def render_scene(kind, theme, cast, state=None, visible=None, tints=None, night=False, incident=False,
                 kanban=None, magnets=None, safe="0", tv=None, balls=0, pizza=0, progress=None, evening=False):
    state, visible, tints = state or {}, visible or {}, tints or {}
    fp = FLOORPLAN["layouts"][kind]
    if progress is not None:
        state, visible = dict(state), dict(visible)
        reads = progress.get("reads", 0)
        state["bookshelf"] = "s3" if reads >= 100 else "s2" if reads >= 50 else "s1" if reads >= 25 else "empty"
        state["test_board"] = "counts"
        for seats in fp["seats"].values():
            for seat in seats:
                n = progress.get("desks", {}).get(seat["desk"], {}).get("monitors", 1)
                for i, m in enumerate(seat.get("monitors", [])):
                    visible[m] = i == 0 or n > 1
    base = Image.new("RGBA", (W, H), (0, 0, 0, 0))
    win = Image.new("RGBA", (W, H), (0, 0, 0, 0))
    world = Image.new("RGBA", (W, H), (0, 0, 0, 0))
    over = Image.new("RGBA", (W, H), (0, 0, 0, 0))
    lamp_glows = []
    # 1. 벽과 바닥 판
    place(base, f"shell/{theme}/default", *rnd(iso(0, 0)))
    # 2. 바닥 타일
    for i in range(COLS):
        for j in range(ROWS):
            fl = ROOMS[room_at(i, j)][5]
            if fl == "boss":
                fl = f"boss_{theme}"
            if fl == "incident":
                fl = "incident_on" if incident else "incident_off"
            place(base, f"tile/{fl}/{'a' if (i + j) % 2 == 0 else 'b'}", *rnd(iso(i, j)))
    items = []
    for pl in fp["placements"]:
        if not visible.get(pl["id"], pl.get("visible", True)):
            continue
        sprite = pl["sprite"]
        if pl.get("themed"):
            sprite = sprite.rsplit("_", 1)[0] + "_" + theme
        key = f"{sprite}/{state.get(pl['id'], pl['state'])}"
        items.append((pl, key))
    # 3. 깔개와 벽 소품, 창문
    for pl, key in items:
        if pl["layer"] in ("decal", "wall"):
            place(base, key, pl["x"], pl["y"])
        elif pl["layer"] == "window":
            place(win, key, pl["x"], pl["y"])
    # 코드가 그리는 벽 위 요소: 칸반 포스트잇, 근태판 자석, TV 그래프
    kb = next(p for p, _ in items if p["id"] == "kanban")
    for slot, color in (kanban or []):
        dx, dy = PROPS[kb["sprite"]]["slots"][slot]
        place(base, f"wall_note/{color}", kb["x"] + dx, kb["y"] + dy)
    at = next(p for p, _ in items if p["id"] == "attendance")
    for col, row, color in (magnets or []):
        dx, dy = PROPS["attendance"]["slots"][col][row]
        place(base, f"magnet/{color}", at["x"] + dx, at["y"] + dy)
    tvp = next(p for p, _ in items if p["id"] == "tv")
    q = PROPS["tv"]["screen"]
    d = ImageDraw.Draw(base)
    for si, series in enumerate(tv or []):
        pts = []
        for k, v in enumerate(series):
            u = k / (len(series) - 1)
            x = tvp["x"] + q["bl"][0] + u * (q["br"][0] - q["bl"][0]) + v * (q["tl"][0] - q["bl"][0])
            y = tvp["y"] + q["bl"][1] + u * (q["br"][1] - q["bl"][1]) + v * (q["tl"][1] - q["bl"][1])
            pts.append(rnd((x, y)))
        d.line(pts, fill=PROPS["tv"]["lineColors"][si])
    if progress is not None:
        board = next(p for p, _ in items if p["id"] == "test_board")
        for row, n in enumerate(progress.get("tests", [0, 0])):
            x, y = board["x"] + 3, board["y"] - 29 + row * 8
            color = (54, 154, 96) if row == 0 else (220, 81, 73)
            d.rectangle([x, y + 1, x + 1, y + 3], fill=color)
            for i, ch in enumerate(str(min(99, n))):
                place(base, f"digit/{ch}", x + 4 + i * 4, y + 2 + i * 2)
            if n > 99:
                d.rectangle([x + 12, y + 5, x + 14, y + 5], fill=color)
                d.rectangle([x + 13, y + 4, x + 13, y + 6], fill=color)
    # 4. 깊이 정렬: 바닥 물건 + 직원
    drawn = []
    for pl, key in items:
        if pl["layer"] in ("world", "fx"):
            drawn.append((pl["depth"], len(drawn), ("prop", pl, key)))
    for c in cast:
        drawn.append((c.get("depth", c["gx"] + c["gy"]), len(drawn), ("owl", c, None)))
    for b in range(balls):
        gx, gy = PROPS["paper_ball"]["slots"][b]
        drawn.append((gx + gy, len(drawn), ("ball", (gx, gy), None)))
    for k in range(pizza):
        st = PROPS["pizza_box"]["stack"]
        drawn.append((24.61 + k * 0.01, len(drawn), ("pizza", (st["gx"], st["gy"], st["z0"] + st["dz"] * k), None)))
    if progress is not None:
        placements = {p["id"]: p for p in fp["placements"]}
        for seats in fp["seats"].values():
            for seat in seats:
                config = progress.get("desks", {}).get(seat["desk"])
                if config is None:
                    continue
                pl = placements[seat["desk"]]
                meta = PROPS[pl["sprite"]]
                for name, key in (("coffee", f"coffee_stack/s{config.get('coffee', 0)}"),
                                  ("papers", f"paper_stack/s{config.get('papers', 0)}"),
                                  ("lamp", "desk_lamp/on" if night else "desk_lamp/off")):
                    if key.endswith("s0"):
                        continue
                    dx, dy = meta["progressSlots"][name]
                    drawn.append((pl["depth"] + 0.03, 5000 + len(drawn), ("dynamic", (pl["gx"] + dx, pl["gy"] + dy, meta["height"]), key)))
                for i in range(len(seat.get("monitors", [])), config.get("monitors", 1)):
                    drawn.append((pl["depth"] + 0.03, 5000 + len(drawn), ("dynamic", (pl["gx"] + 0.05 + i * 0.65, pl["gy"] + 0.05, meta["height"]), "monitor_sm/off")))
                if night:
                    dx, dy = meta["progressSlots"]["lamp"]
                    lamp_glows.append(rnd(iso(pl["gx"] + dx, pl["gy"] + dy, meta["height"])))
        for gx, gy in [(3.9, 14.6), (3.9, 15.4), (1.5, 15.3)][:min(3, progress.get("commits", 0))]:
            drawn.append((gx + gy + 0.65, 5000 + len(drawn), ("dynamic", (gx, gy, 0), "box_taped/default")))
        if progress.get("pr"):
            drawn.append((80, 5000, ("dynamic", (3.5, 16.6, 0), "truck/pr")))
    owl_pos = {}
    for _, _, (kind_, obj, key) in sorted(drawn, key=lambda t: (t[0], t[1])):
        if kind_ == "prop":
            tint = tints.get(obj["id"])
            place(world, key, obj["x"], obj["y"], tint=tint)
            if obj["id"] == "safety_board":
                ox, oy = PROPS["safety_board"]["digitOrigin"]
                sx, sy = PROPS["safety_board"]["digitStep"]
                for n, ch in enumerate(safe):
                    place(world, f"digit/{ch}", obj["x"] + ox + n * sx, obj["y"] + oy + n * sy)
        elif kind_ == "dynamic":
            place(world, key, *rnd(iso(*obj)))
        elif kind_ == "owl":
            owl_pos[obj["id"]] = draw_owl(world, obj)
        elif kind_ == "ball":
            place(world, "paper_ball/default", *rnd(iso(*obj)))
        elif kind_ == "pizza":
            place(world, "pizza_box/default", *rnd(iso(*obj)))
    for pl, key in items:
        if pl["layer"] == "overlay":
            place(over, key, pl["x"], pl["y"])
    for x, y in lamp_glows:
        place(over, "glow_lamp/default", x, y)
    if night or evening:
        tint = NIGHT_TINT if night else EVENING_TINT
        base = tinted(base, tint)
        world = tinted(world, tint)
    out = base
    out.alpha_composite(win)
    out.alpha_composite(world)
    out.alpha_composite(over)
    return out, owl_pos


# =====================================================================
# UI 시트: 로고, 로딩 건물, 규모 미리보기
# =====================================================================
NAVY, LIT, DARKWIN = (38, 42, 62), (255, 208, 102), (62, 68, 94)
FONT5 = {"O": [".##.", "#..#", "#..#", "#..#", ".##."], "W": ["#...#", "#...#", "#.#.#", "#.#.#", ".#.#."],
         "L": ["#..", "#..", "#..", "#..", "###"], "F": ["###", "#..", "##.", "#..", "#.."],
         "I": ["###", ".#.", ".#.", ".#.", "###"], "C": [".##", "#..", "#..", "#..", ".##"],
         "E": ["###", "#..", "##.", "#..", "###"]}


def text_px(d, x, y, s, color):
    cx = x
    for ch in s:
        g = FONT5[ch]
        for gy_, row in enumerate(g):
            for gx_, c in enumerate(row):
                if c == "#":
                    d.point((cx + gx_, y + gy_), fill=color)
        cx += len(g[0]) + 1


BUILDING_WINDOWS = []   # 문을 뺀 창문 왼쪽 위 좌표, 행 우선
BUILDING_CR = []


def building(lit_ratio=0.0, eyes=0, sign=False, w=160, h=150, base_only=False):
    img = Image.new("RGBA", (w, h), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    bx0, bx1, by0, by1 = 28, w - 28, 34, h - 8
    d.rectangle([bx0, by0, bx1, by1], fill=NAVY)
    d.rectangle([bx0 - 3, by0 - 3, bx1 + 3, by0], fill=(30, 33, 50))
    sx0, sx1 = w // 2 - 22, w // 2 + 22
    d.rectangle([sx0, 12, sx1, 26], fill=(30, 33, 50))
    d.line([(sx0 + 6, 26), (sx0 + 6, by0 - 3)], fill=(30, 33, 50))
    d.line([(sx1 - 6, 26), (sx1 - 6, by0 - 3)], fill=(30, 33, 50))
    if sign:
        d.rectangle([sx0 - 1, 11, sx1 + 1, 27], outline=(255, 190, 120))
    text_px(d, sx0 + 7, 17, "OWLFICE", (255, 150, 70) if sign else (78, 84, 108))
    cols, rows, ww, wh = 5, 6, 10, 9
    gapx = (bx1 - bx0 - cols * ww) // (cols + 1)
    order = sorted([(c, r) for r in range(rows) for c in range(cols)], key=lambda t: (t[1] * 7 + t[0] * 3) % 11)
    n_lit = int(len(order) * lit_ratio)
    lit_set, eye_set = set(order[:n_lit]), set(order[n_lit:n_lit + eyes])
    BUILDING_WINDOWS.clear()
    BUILDING_CR.clear()
    for r in range(rows):
        for c in range(cols):
            x = bx0 + gapx + c * (ww + gapx)
            y = by0 + 8 + r * (wh + 6)
            if r == rows - 1 and c == cols // 2:
                d.rectangle([x - 1, y - 2, x + ww, by1], fill=(24, 26, 40))
                continue
            BUILDING_WINDOWS.append([x, y])
            BUILDING_CR.append((c, r))
            if (c, r) in lit_set and not base_only:
                d.rectangle([x, y, x + ww - 1, y + wh - 1], fill=LIT)
                d.line([(x, y + wh - 1), (x + ww - 1, y + wh - 1)], fill=(230, 170, 70))
            else:
                d.rectangle([x, y, x + ww - 1, y + wh - 1], fill=DARKWIN)
                if (c, r) in eye_set and not base_only:
                    d.rectangle([x + 2, y + 3, x + 3, y + 4], fill=(255, 214, 80))
                    d.rectangle([x + 6, y + 3, x + 7, y + 4], fill=(255, 214, 80))
    d.line([(4, h - 8), (w - 5, h - 8)], fill=(200, 200, 196))
    return img


def window_order():
    """BUILDING_WINDOWS 인덱스를 불 켜는 순서대로."""
    order = sorted([(c, r) for r in range(6) for c in range(5)], key=lambda t: (t[1] * 7 + t[0] * 3) % 11)
    return [BUILDING_CR.index(t) for t in order if t in BUILDING_CR]


def preview_large():
    w, h = 132, 72
    img = Image.new("RGBA", (w, h), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    for i, (x, top, bw) in enumerate([(6, 22, 34), (46, 8, 40), (92, 18, 34)]):
        d.rectangle([x, top, x + bw, h - 6], fill=NAVY)
        for r in range((h - 6 - top - 6) // 9):
            for c in range(3):
                wx, wy = x + 4 + c * 10 + (2 if bw == 40 else 0), top + 5 + r * 9
                d.rectangle([wx, wy, wx + 6, wy + 5], fill=LIT if (i == 1 and r == 2 and c == 1) else DARKWIN)
    d.line([(0, h - 6), (w - 1, h - 6)], fill=(200, 200, 196))
    return img


def preview_small():
    w, h = 132, 72
    img = Image.new("RGBA", (w, h), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    d.rectangle([22, 14, 110, h - 6], fill=NAVY)
    d.rectangle([20, 11, 112, 14], fill=(30, 33, 50))
    for r in range(5):
        for c in range(7):
            wx, wy = 27 + c * 12, 19 + r * 10
            d.rectangle([wx, wy, wx + 7, wy + 6], fill=LIT if r == 2 else DARKWIN)
    d.line([(0, h - 6), (w - 1, h - 6)], fill=(200, 200, 196))
    return img


def logo():
    rows = ["..k........k..", ".kbk......kbk.", ".kbbkkkkkkbbk.", "kbbbbbbbbbbbbk", "kbllllbbllllbk",
            "kblyylbblyylbk", "kblypybbypylbk", "kblllloollllbk", "kbbbbboobbbbbk", ".kbbbbbbbbbbk.", "..kkkkkkkkkk.."]
    pal = dict(k=(28, 24, 34), b=(60, 52, 70), l=(98, 88, 110), y=(255, 196, 60), p=(26, 26, 26), o=(255, 150, 60))
    img = Image.new("RGBA", (16, 13), (0, 0, 0, 0))
    img.alpha_composite(pix(rows, pal), (1, 1))
    return img


def build_ui():
    ui = {}
    ui["logo/default"] = dict(img=logo(), anchor=[0, 0])
    ui["building/base"] = dict(img=building(base_only=True), anchor=[0, 0])
    ui["building/sign_on"] = dict(img=building(sign=True, base_only=True).crop((56, 10, 105, 29)), anchor=[0, 0])
    lit = Image.new("RGBA", (10, 9), LIT + (255,))
    ImageDraw.Draw(lit).line([(0, 8), (9, 8)], fill=(230, 170, 70))
    ui["building/window_lit"] = dict(img=lit, anchor=[0, 0])
    eye = Image.new("RGBA", (10, 9), DARKWIN + (255,))
    ImageDraw.Draw(eye).rectangle([2, 3, 3, 4], fill=(255, 214, 80))
    ImageDraw.Draw(eye).rectangle([6, 3, 7, 4], fill=(255, 214, 80))
    ui["building/window_eyes"] = dict(img=eye, anchor=[0, 0])
    ui["preview/large"] = dict(img=preview_large(), anchor=[0, 0])
    ui["preview/small"] = dict(img=preview_small(), anchor=[0, 0])
    building(base_only=True)
    extra = dict(building=dict(size=[160, 150], windows=list(BUILDING_WINDOWS), lightOrder=window_order(),
                               signOffset=[56, 10],
                               rule="로딩 단계 n/7 마다 lightOrder 앞에서부터 창 불을 켠다 (window_lit). 켜지지 않은 창 중 앞의 몇 개에 올빼미 눈 (window_eyes). 7단계가 끝나면 sign_on"))
    return ui, extra


# =====================================================================
# 시트 쓰기
# =====================================================================
def pack(entries, max_w):
    items = sorted(entries.items(), key=lambda kv: (-kv[1]["img"].height, kv[0]))
    x = y = row_h = 0
    pos = {}
    for k, e in items:
        w, h = e["img"].size
        if x + w > max_w:
            x, y, row_h = 0, y + row_h + 1, 0
        pos[k] = (x, y)
        x += w + 1
        row_h = max(row_h, h)
    total = y + row_h
    hh = 1
    while hh < total:
        hh *= 2
    sheet = Image.new("RGBA", (max_w, hh), (0, 0, 0, 0))
    for k, e in items:
        sheet.alpha_composite(e["img"], pos[k])
    return sheet, pos


def write_atlas(name, entries, animations, extra, max_w):
    sheet, pos = pack(entries, max_w)
    frames = {}
    for k in sorted(entries):
        e = entries[k]
        x, y = pos[k]
        w, h = e["img"].size
        ax, ay = e["anchor"]
        frames[k] = {"frame": {"x": x, "y": y, "w": w, "h": h}, "rotated": False, "trimmed": False,
                     "spriteSourceSize": {"x": 0, "y": 0, "w": w, "h": h}, "sourceSize": {"w": w, "h": h},
                     "anchor": {"x": round(ax / w, 6), "y": round(ay / h, 6)}, "anchorPx": [ax, ay]}
    data = {"frames": frames, "animations": animations,
            "meta": {"app": "design/tools/owlfice_art.py", "version": "1", "image": f"{name}.png", "format": "RGBA8888",
                     "size": {"w": sheet.width, "h": sheet.height}, "scale": "1"},
            "owlfice": extra}
    sheet.save(os.path.join(SPR, f"{name}.png"))
    with open(os.path.join(SPR, f"{name}.json"), "w", encoding="utf-8", newline="\n") as f:
        json.dump(data, f, ensure_ascii=False, indent=1)
    return sheet, pos


# =====================================================================
# 미리보기와 캔버스용 참조 렌더
# =====================================================================
def owl_composite(species, outfit, role="trainee", pose="idle", dirn="f", frame=0, held=None, wear=0):
    img = Image.new("RGBA", (OWL_W + 8, OWL_H), (0, 0, 0, 0))
    draw_owl_local(img, dict(species=species, outfit=outfit, role=role, pose=pose, dir=dirn, frame=frame, held=held, wear=wear), 4 + OWL_AX, OWL_AY)
    return img


def draw_owl_local(img, c, x, y):
    keys, name = owl_keys(c)
    for k in keys:
        if k in OWLS:
            place(img, k, x, y)
    if c.get("held"):
        hx, hy = OWL_META["anchors"][name]["hand"]
        place(img, f"held_{c['held']}/default", x - OWL_AX + hx, y - OWL_AY + hy)


SHOWCASE = [  # 5종 × 4직급 = 20
    ("eagle", "claude", ["backend", "devops", "frontend", "trainee"]),
    ("longeared", "codex", ["qa", "backend", "writer", "frontend"]),
    ("little", "gemini", ["frontend", "qa", "trainee", "writer"]),
    ("barn", "claude", ["devops", "writer", "qa", "backend"]),
    ("scops", "codex", ["writer", "frontend", "backend", "qa"]),
]


def make_previews():
    os.makedirs(PREV, exist_ok=True)
    cw, ch = 24, 28
    sheet = Image.new("RGBA", (cw * 4, ch * 5), (0, 0, 0, 0))
    labels = []
    for r, (sp, vendor, roles) in enumerate(SHOWCASE):
        for c, rank in enumerate(("bujang", "gwajang", "daeri", "sawon")):
            img = owl_composite(sp, f"{rank}-{vendor}", roles[c])
            sheet.alpha_composite(img, (c * cw, r * ch + 2))
            labels.append(dict(species=SPECIES[sp]["name"], rank=RANK_KO[rank], role=ROLE_KO[roles[c]], vendor=THEMES[vendor]["label"]))
    sheet.save(os.path.join(PREV, "owls-20.png"))
    poses = [(p, "f", f) for p, d_, f in pose_frames() if d_ == "f"] + [(p, "b", f) for p, d_, f in pose_frames() if d_ == "b"]
    strip = Image.new("RGBA", (24 * 12, 28 * ((len(poses) + 11) // 12)), (0, 0, 0, 0))
    for i, (pose, dirn, f) in enumerate(poses):
        held = "clipboard" if pose == "carry" else None
        img = owl_composite("eagle", "bujang-claude", "backend", pose, dirn, f, held)
        strip.alpha_composite(img, ((i % 12) * 24, (i // 12) * 28 + 2))
    strip.save(os.path.join(PREV, "poses.png"))
    layers = Image.new("RGBA", (24 * 4, 28), (0, 0, 0, 0))
    for i, keys in enumerate([["body/barn/idle_f_0"], ["outfit/gwajang-claude/idle_f_0"], ["role/qa/idle_f_0"],
                              ["body/barn/idle_f_0", "outfit/gwajang-claude/idle_f_0", "role/qa/idle_f_0"]]):
        for k in keys:
            place(layers, k, i * 24 + 4 + OWL_AX, 2 + OWL_AY)
    layers.save(os.path.join(PREV, "layers.png"))
    with open(os.path.join(PREV, "owls-20.json"), "w", encoding="utf-8", newline="\n") as f:
        json.dump(dict(cell=[cw, ch], cols=4, labels=labels, poses=[f"{p}_{d_}_{f_}" for p, d_, f_ in poses]), f, ensure_ascii=False, indent=1)
    wear_sheet = Image.new("RGBA", (24 * 4, 28 * 4), (0, 0, 0, 0))
    for row, outfit in enumerate(("bujang-claude", "gwajang-codex", "daeri-gemini", "sawon-claude")):
        for stage in range(4):
            wear_sheet.alpha_composite(owl_composite("barn", outfit, wear=stage), (stage * 24, row * 28 + 2))
    wear_sheet.save(os.path.join(PREV, "wear.png"))
    roles = Image.new("RGBA", (24 * 3, 28 * 5), (0, 0, 0, 0))
    for row, sp in enumerate(EMPLOYEE_SPECIES):
        for col, role in enumerate(("security", "data", "designer")):
            roles.alpha_composite(owl_composite(sp, "bujang-claude", role), (col * 24, row * 28 + 2))
    roles.save(os.path.join(PREV, "roles.png"))
    items = Image.new("RGBA", (24 * 6, 28), (0, 0, 0, 0))
    for col, equipped in enumerate((["medal"], ["helmet"], ["blanket"], ["headphones"], ["helmet", "headphones"], ["blanket", "medal"])):
        draw_owl_local(items, dict(species="barn", outfit="bujang-claude", role="designer", pose="idle", items=equipped, wear=3), col * 24 + 4 + OWL_AX, OWL_AY)
    items.save(os.path.join(PREV, "items.png"))
    # 소품 목록 (3배, 이름 표시)
    scale, pad, label_h = 3, 14, 18
    font = ImageFont.truetype("consola.ttf", 13) if os.path.exists(r"C:\Windows\Fonts\consola.ttf") else ImageFont.load_default()
    keys = [k for k in sorted(OFFICE) if not k.startswith(("shell/", "tile/", "wallseg_", "glow_"))]
    width = 1600
    x = y = row_h = 0
    places = []
    for k in keys:
        img = OFFICE[k]["img"]
        w = max(img.width * scale, int(font.getlength(k)) + 4)
        h = img.height * scale + label_h
        if x + w > width:
            x, y, row_h = 0, y + row_h + pad, 0
        places.append((k, x, y, w))
        x += w + pad
        row_h = max(row_h, h)
    out = Image.new("RGBA", (width, y + row_h + 4), (255, 255, 255, 255))
    d = ImageDraw.Draw(out)
    for k, x, y, w in places:
        img = OFFICE[k]["img"]
        big = img.resize((img.width * scale, img.height * scale), Image.NEAREST)
        out.alpha_composite(big, (x, y))
        d.text((x, y + big.height + 3), k, fill=(82, 82, 91), font=font)
    out.save(os.path.join(PREV, "props-sheet.png"))
    return labels


def make_canvas_images():
    """캔버스 시안이 쓰는 그림. 모두 시트와 평면도로만 그린다."""
    hero_cast = [
        dict(id="bujang", wear=1, species="eagle", outfit="bujang-claude", role="backend", pose="carry", gx=20.9, gy=2.15, depth=23.0, held="duck"),
        dict(id="gwajang", wear=2, species="barn", outfit="gwajang-claude", role="backend", pose="idle", gx=13.3, gy=2.75),
        dict(id="daeri", wear=3, species="little", outfit="daeri-claude", role="frontend", pose="type", gx=2.75, gy=6.25, depth=8.9),
        dict(id="sawon1", species="scops", outfit="sawon-claude", role="trainee", pose="carry", gx=1.4, gy=1.7, held="folder"),
        dict(id="sawon2", species="longeared", outfit="sawon-claude", role="qa", pose="idle", gx=10.5, gy=12.6),
        dict(id="librarian", species="little", outfit="librarian", role="trainee", pose="idle", gx=2.8, gy=2.95, depth=5.75),
    ]
    kan = [(0, "yellow"), (1, "pink"), (2, "yellow"), (3, "blue"), (4, "yellow"), (6, "green"), (7, "green"), (8, "green")]
    mags = [(0, 0, "red"), (0, 1, "blue"), (0, 2, "green"), (0, 3, "yellow"), (3, 0, "gray")]
    tv1 = [[0.1, 0.18, 0.14, 0.3, 0.36, 0.46, 0.42, 0.56, 0.2, 0.26, 0.36, 0.42]]
    hero_args = dict(state={"ci_light": "yellow", "cabinet_front_1": "open", "boss_mon_0_0": "code_0", "boss_mon_0_1": "read_0",
                                        "dev_mon_0_0": "code_0", "dev_mon_0_1": "code_1", "plant_boss_0": "s3", "trash": "some",
                                        "mug_0": "steam_0", "test_machine": "run_0", "test_board": "pass"},
                                 visible={"duck_0": False}, tints={f"dev_part_{i}": THEMES["claude"]["cubicle"] for i in range(6)},
                                 kanban=kan, magnets=mags, safe="37", tv=tv1, balls=2,
                                 progress=dict(desks={"boss_desk_0": dict(coffee=2, papers=1, monitors=2)}, reads=50, commits=2, pr=True, tests=[12, 2]))
    img, pos_hero = render_scene("large", "claude", hero_cast, **hero_args)
    img.save(os.path.join(CANVAS, "office-claude-day.png"))
    evening_args = {**hero_args, "state": {**hero_args["state"], "window": "dusk"}}
    evening, _ = render_scene("large", "claude", hero_cast, evening=True, **evening_args)
    evening.save(os.path.join(CANVAS, "office-claude-evening.png"))
    T = TEAM_TINTS
    smb_cast = [
        dict(id="bujangA", wear=1, species="scops", outfit="bujang-codex", role="devops", pose="idle", gx=17.6, gy=4.1),
        dict(id="bujangB", wear=2, species="eagle", outfit="bujang-codex", role="frontend", pose="jump", gx=19.6, gy=2.2, depth=21.6),
        dict(id="bujangC", wear=3, species="longeared", outfit="bujang-codex", role="backend", pose="panic", gx=19.6, gy=9.95, depth=29.7),
        dict(id="subC1", species="barn", outfit="daeri-codex", role="qa", pose="idle", gx=21.8, gy=10.8),
        dict(id="subC2", species="little", outfit="sawon-codex", role="trainee", pose="idle", gx=22.8, gy=10.3),
        dict(id="subA1", species="little", outfit="daeri-codex", role="frontend", pose="type", gx=2.75, gy=6.25, depth=8.9),
        dict(id="subB1", species="barn", outfit="gwajang-codex", role="backend", pose="read", gx=6.15, gy=6.25, depth=12.3),
        dict(id="subA2", species="scops", outfit="sawon-codex", role="trainee", pose="sleep", gx=12.2, gy=13.9, z=6, depth=25.6),
        dict(id="boss", species="snowy", outfit="boss", role="trainee", pose="walk", dir="b", frame=1, gx=13.0, gy=8.2),
    ]
    tints = {f"dev_part_{i}": c for i, c in enumerate([T[0], T[1], T[2], T[2], THEMES["codex"]["cubicle"], THEMES["codex"]["cubicle"]])}
    img, pos_smb = render_scene("small", "codex", smb_cast, night=True, incident=True,
                                state={"ci_light": "red", "boss_mon_0_0": "saver_0", "boss_mon_1_0": "code_0", "dev_mon_0_0": "code_0",
                                       "dev_mon_0_1": "code_1", "dev_mon_1_0": "read_0", "dev_mon_1_1": "read_1", "plant_boss_0": "s2",
                                       "plant_boss_1": "s4", "plant_boss_2": "s4", "trash": "full", "whiteboard": "scribble",
                                       "beacon": "on_0", "incident_table": "on", "interphone": "ring_0", "window": "night",
                                       "test_board": "fail"},
                                visible={"ramen": True, "glow_incident": True, "plant_boss_0": True, "plant_boss_1": True, "plant_boss_2": True},
                                tints=tints, kanban=kan[:6], magnets=mags + [(1, 0, "teal"), (1, 1, "violet")], safe="0",
                                tv=[[0.1, 0.18, 0.14, 0.3, 0.36, 0.46, 0.42, 0.56, 0.2, 0.26, 0.36, 0.42],
                                    [0.0, 0.06, 0.06, 0.12, 0.24, 0.24, 0.3, 0.36, 0.42, 0.42, 0.48, 0.6]], balls=5, pizza=2,
                                progress=dict(desks={"boss_desk_0": dict(coffee=4, papers=4, monitors=2, night=True)}, reads=100, commits=3, tests=[105, 7]))
    img.save(os.path.join(CANVAS, "office-codex-night-smb.png"))
    for t in ("codex", "gemini"):
        cast = [dict(id="bujang", species="eagle", outfit=f"bujang-{t}", role="backend", pose="idle", gx=20.9, gy=2.15, depth=23.0),
                dict(id="daeri", species="little", outfit=f"daeri-{t}", role="frontend", pose="type", gx=2.75, gy=6.25, depth=8.9),
                dict(id="sawon", species="barn", outfit=f"sawon-{t}", role="qa", pose="read", gx=2.75, gy=8.85, depth=11.5)]
        img, _ = render_scene("large", t, cast, state={"ci_light": "green", "boss_mon_0_0": "code_0", "dev_mon_0_0": "code_0",
                                                        "dev_mon_0_1": "code_1", "dev_mon_3_0": "read_0", "plant_boss_0": "s1", "trash": "some"},
                              tints={f"dev_part_{i}": THEMES[t]["cubicle"] for i in range(6)}, kanban=kan[:5], magnets=mags[:3],
                              safe="12", tv=tv1, balls=1,
                              progress=dict(desks={"boss_desk_0": dict(coffee=1, papers=2)}, reads=25, commits=1, tests=[3, 0]))
        img.save(os.path.join(CANVAS, f"office-{t}-day.png"))
    # 카드 아바타 (20×22, 2배로 40×44 표시)
    avatars = {
        "claude-bujang": ("eagle", "bujang-claude", "backend"), "claude-gwajang": ("barn", "gwajang-claude", "backend"),
        "claude-daeri": ("little", "daeri-claude", "frontend"), "claude-sawon": ("scops", "sawon-claude", "trainee"),
        "claude-sawon-qa": ("longeared", "sawon-claude", "qa"),
        "codex-bujang": ("longeared", "bujang-codex", "backend"), "codex-bujang-front": ("eagle", "bujang-codex", "frontend"),
        "codex-gwajang": ("barn", "gwajang-codex", "backend"), "codex-daeri": ("barn", "daeri-codex", "qa"),
        "codex-sawon": ("little", "sawon-codex", "trainee"),
        "gemini-bujang": ("scops", "bujang-gemini", "devops"), "gemini-gwajang": ("longeared", "gwajang-gemini", "frontend"),
        "gemini-daeri": ("little", "daeri-gemini", "writer"), "gemini-sawon": ("barn", "sawon-gemini", "qa"),
        "boss": ("snowy", "boss", "trainee"),
    }
    for name, (sp, of, role) in avatars.items():
        full = Image.new("RGBA", (OWL_W, OWL_H), (0, 0, 0, 0))
        draw_owl_local(full, dict(species=sp, outfit=of, role=role, pose="idle"), OWL_AX, OWL_AY)
        av = Image.new("RGBA", (20, 22), (0, 0, 0, 0))
        av.alpha_composite(full.crop((0, 2, 16, 24)), (2, 0))
        av.save(os.path.join(CANVAS, f"avatar-{name}.png"))
    with open(os.path.join(SPR, "preview", "positions.json"), "w", encoding="utf-8", newline="\n") as f:
        json.dump(dict(hero=pos_hero, smb=pos_smb), f, ensure_ascii=False, indent=1)
    for name, im in (("building-loading", building(0.55, 3)), ("building-dark", building(0.0)), ("preview-large", preview_large()),
                     ("preview-small", preview_small()), ("logo", logo())):
        im.save(os.path.join(CANVAS, f"{name}.png"))


# =====================================================================
# 검사
# =====================================================================
def check(owl_sheet_pos, office_pos):
    errors = []
    for name, pos, entries in (("owls", owl_sheet_pos, OWLS), ("office", office_pos, OFFICE)):
        occ = Image.new("L", (4096, 4096), 0)
        for k, e in entries.items():
            x, y = pos[k]
            w, h = e["img"].size
            if occ.crop((x, y, x + w, y + h)).getbbox() is not None:
                errors.append(f"{name}: {k} 자리가 다른 프레임과 겹침")
            occ.paste(255, (x, y, x + w, y + h))
    expected = sum(spec.get("f", 0) + spec.get("b", 0) for spec in POSES.values())
    for sp in SPECIES:
        n = sum(1 for k in OWLS if k.startswith(f"body/{sp}/"))
        if n != expected:
            errors.append(f"body/{sp}: 프레임 {n}개, 기대 {expected}개")
    for i in range(COLS):
        for j in range(ROWS):
            matches = [k for k, (_, a, b, c, d, _) in ROOMS.items() if a <= i < c and b <= j < d]
            if len(matches) != 1:
                errors.append(f"바닥 ({i},{j}) 방 중복·누락: {matches}")
    for kind, lay in FLOORPLAN["layouts"].items():
        for pl in lay["placements"]:
            for st in PROPS[pl["sprite"]]["states"]:
                key = f"{pl['sprite']}/{st}"
                if pl.get("themed"):
                    key = f"{pl['sprite'].rsplit('_', 1)[0]}_claude/{st}"
                if key not in OFFICE:
                    errors.append(f"{kind}: {pl['id']} 의 {key} 없음")
        for group, ss in lay["seats"].items():
            for s in ss:
                if not any(a <= s["gx"] < c and b <= s["gy"] < e for _, a, b, c, e, _ in ROOMS.values()):
                    errors.append(f"{kind}: 좌석 {s['id']} 가 방 밖")
    for name in ("idle_f_0", "walk_b_3", "carry_f_0"):
        hx, hy = OWL_META["anchors"][name]["hand"]
        if not (0 <= hx < OWL_W and 0 <= hy < OWL_H):
            errors.append(f"손 기준점 {name} 이 프레임 밖")
    for role in ("security", "data", "designer"):
        frame = OWLS[f"role/{role}/idle_f_0"]["img"]
        if frame.crop((0, 2, 16, 24)).getbbox() is None or frame.crop((0, 0, 16, 2)).getbbox() is not None:
            errors.append(f"role/{role}: 카드 자르기 안에 담당 그림이 온전히 들어오지 않음")
    assert not errors, "\n".join(errors)
    print(f"검사 통과: 올빼미 {len(OWLS)} 프레임, 사무실 {len(OFFICE)} 프레임, 소품 {len(PROPS)} 종")


# =====================================================================
OWLS, OWL_META, FLOORPLAN = {}, {}, {}


def main():
    os.makedirs(SPR, exist_ok=True)
    os.makedirs(CANVAS, exist_ok=True)
    frames, anchors = build_owls()
    for k, img in frames.items():
        if img.getbbox() is None and not k.startswith("body/"):
            continue   # 빈 레이어 프레임은 시트에 넣지 않는다 (코드는 없으면 건너뛴다)
        OWLS[k] = dict(img=img, anchor=[OWL_AX, OWL_AY])
    OWL_META.update(anchors=anchors)
    anims = {}
    for k in OWLS:
        group = k.rsplit("_", 1)[0]
        anims.setdefault(group, []).append(k)
    for g in anims:
        anims[g].sort(key=lambda s: int(s.rsplit("_", 1)[1]))
    owl_extra = dict(
        frame=[OWL_W, OWL_H], anchor=[OWL_AX, OWL_AY],
        layers=["body", "outfit", "wear", "role", "item", "held"],
        layerRule="body → outfit → wear → role → item 순서로 같은 위치에 겹친다. role 프레임이 없으면 건너뛴다 (그 자세에선 소품이 안 보임). held 는 office 시트의 held_* 를 hand 기준점에",
        wear={"1": "1시간 · 소매 걷음·부장 넥타이 느슨", "2": "3시간 · 눈 밑 그림자 추가", "3": "6시간 · 어깨 수건 추가"},
        species={k: v["name"] for k, v in SPECIES.items()}, employeeSpecies=EMPLOYEE_SPECIES,
        speciesRule="직원 종 = EMPLOYEE_SPECIES[hash(agent_id) % 5]. 흰올빼미는 사장님 전용",
        outfits={o: (RANK_KO.get(o.split('-')[0]) + (' · ' + THEMES[o.split('-')[1]]['label'] if '-' in o else '')) for o in OUTFITS},
        roles={r: ROLE_KO[r] for r in ROLES}, trainee="수습은 role 레이어 없음",
        poses={p: dict(dirs={d_: s.get(d_, 0) for d_ in ("f", "b") if s.get(d_)}, fps=s["fps"], loop=s["loop"], note=s["note"])
               for p, s in POSES.items()},
        anchors=anchors,
        anchorNote="hand = 오른쪽 날개 끝 (held 물건 기준점을 여기에), head = 머리 윗선 가운데 (말풍선·배지 기준). 프레임 좌표 px",
    )
    owl_sheet, owl_pos = write_atlas("owls", OWLS, anims, owl_extra, 1024)
    build_tiles()
    build_shell()
    build_wall_items()
    build_furniture()
    build_fx()
    build_wallsegs()
    office_anims = {}
    for k in OFFICE:
        name, st = k.split("/", 1)
        office_anims.setdefault(name, []).append(k)
    FLOORPLAN.update(floorplan_json())
    office_sheet, office_pos = write_atlas("office", OFFICE, {}, dict(props=PROPS), 2048)
    ui, ui_extra = build_ui()
    write_atlas("ui", ui, {}, ui_extra, 512)
    with open(os.path.join(SPR, "floorplan.json"), "w", encoding="utf-8", newline="\n") as f:
        json.dump(FLOORPLAN, f, ensure_ascii=False, indent=1)
    make_previews()
    make_canvas_images()
    check(owl_pos, office_pos)
    print(f"owls.png {owl_sheet.size}, office.png {office_sheet.size}")


if __name__ == "__main__":
    main()
