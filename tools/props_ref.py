# /// script
# requires-python = ">=3.11"
# dependencies = ["pillow"]
# ///
"""T23 소품 상태 참조 그림. 직원 없이 소품 상태만 바꾼 두 장을 dev/ref/ 에 쓴다.

호출: uv run tools/props_ref.py
dev/props.ts 의 CASES 와 같은 상태여야 한다. TV 선은 그림에 넣지 않는다 (지금 값 점 하나라 선과 달라서 눈으로 본다).
"""

import os
import sys

from PIL import Image, ImageDraw

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.dont_write_bytecode = True
sys.path.insert(0, os.path.join(ROOT, "design", "tools"))

import owlfice_art as art

OUT = os.path.join(ROOT, "dev", "ref")

# (이름, kind, theme, night, 상태, 보임, 포스트잇 (슬롯, 색), 전광판, 종이 수, 피자 수, 통계)
CASES = [
    (
        "props-large-claude",
        "large",
        "claude",
        False,
        {"ci_light": "green", "plant_boss_0": "s3", "trash": "full", "window": "dusk"},
        {},
        [(0, "yellow"), (1, "pink"), (3, "blue"), (6, "green"), (7, "green")],
        "37",
        3,
        2,
        (1, 2, 3, 4),
    ),
    (
        "props-small-codex-night",
        "small",
        "codex",
        True,
        {
            "ci_light": "red",
            "plant_boss_0": "s2",
            "plant_boss_1": "s4",
            "plant_boss_3": "wilted",
            "trash": "full",
            "window": "night",
        },
        {
            "ramen": True,
            "plant_boss_0": True,
            "plant_boss_1": True,
            "plant_boss_3": True,
        },
        [(0, "yellow"), (3, "blue"), (4, "blue"), (6, "green")],
        "1234",
        10,
        8,
        (0, 9, 10, 12345),
    ),
]
CASES.append(("props-large-claude-evening", *CASES[0][1:]))


def stats_board_sprite(placement, stats):
    canvas = Image.new("RGBA", (art.W, art.H), (0, 0, 0, 0))
    x, y = placement["x"], placement["y"]
    art.place(canvas, "stats_board/default", x, y)
    draw = ImageDraw.Draw(canvas)
    draw.polygon(
        [(x - 1, y - 28), (x - 13, y - 22), (x - 13, y - 9), (x - 1, y - 15)],
        fill=(40, 70, 56, 255),
    )
    for i, n in enumerate(stats):
        dx = x - 12 + (i % 2) * 6
        dy = y - 21 - (i % 2) * 3 + (i // 2) * 7
        if n > 9:
            draw.rectangle((dx, dy + 2, dx + 2, dy + 2), fill=(220, 230, 220, 255))
            draw.rectangle((dx + 1, dy + 1, dx + 1, dy + 3), fill=(220, 230, 220, 255))
        else:
            art.place(canvas, f"digit/{n}", dx, dy)
    bounds = canvas.getbbox()
    return {"img": canvas.crop(bounds), "anchor": [x - bounds[0], y - bounds[1]]}


def main():
    art.build_tiles()
    art.build_shell()
    art.build_wall_items()
    art.build_furniture()
    art.build_fx()
    art.build_wallsegs()
    art.FLOORPLAN.update(art.floorplan_json())
    os.makedirs(OUT, exist_ok=True)
    original_board = art.OFFICE["stats_board/default"]
    for (
        name,
        kind,
        theme,
        night,
        state,
        visible,
        kanban,
        safe,
        balls,
        pizza,
        stats,
    ) in CASES:
        cubicle = art.THEMES[theme]["cubicle"]
        placement = next(
            p
            for p in art.FLOORPLAN["layouts"][kind]["placements"]
            if p["id"] == "stats_board"
        )
        art.OFFICE["stats_board/default"] = stats_board_sprite(placement, stats)
        try:
            img, _ = art.render_scene(
                kind,
                theme,
                [],
                state=state,
                visible=visible,
                tints={f"dev_part_{i}": cubicle for i in range(6)},
                night=night,
                evening=name.endswith("evening"),
                kanban=kanban,
                safe=safe,
                balls=balls,
                pizza=pizza,
                magnets=[(0, 0, "teal" if night else "red")],
                progress={
                    "desks": {
                        "boss_desk_0": {
                            "coffee": 4 if night else 2,
                            "papers": 2,
                            "monitors": 2,
                            "night": night,
                        },
                    },
                    "reads": 100 if night else 50,
                    "commits": 2,
                    "pr": True,
                    "tests": [105, 7] if night else [12, 2],
                },
            )
        finally:
            art.OFFICE["stats_board/default"] = original_board
        path = os.path.join(OUT, f"{name}.png")
        img.save(path)
        print(path)


if __name__ == "__main__":
    main()
