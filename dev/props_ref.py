# /// script
# requires-python = ">=3.11"
# dependencies = ["pillow"]
# ///
"""T23 소품 상태 참조 그림. 직원 없이 소품 상태만 바꾼 두 장을 dev/ref/ 에 쓴다.

호출: uv run dev/props_ref.py
dev/props.ts 의 CASES 와 같은 상태여야 한다. TV 선은 그림에 넣지 않는다 (지금 값 점 하나라 선과 달라서 눈으로 본다).
"""

import os
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.dont_write_bytecode = True
sys.path.insert(0, os.path.join(ROOT, "design", "tools"))

import owlfice_art as art

OUT = os.path.join(ROOT, "dev", "ref")

# (이름, kind, theme, night, 상태, 보임, 포스트잇 (슬롯, 색), 전광판, 종이 수, 피자 수)
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
        {"ramen": True, "plant_boss_0": True, "plant_boss_1": True, "plant_boss_3": True},
        [(0, "yellow"), (3, "blue"), (4, "blue"), (6, "green")],
        "1234",
        10,
        8,
    ),
]


def main():
    art.build_tiles()
    art.build_shell()
    art.build_wall_items()
    art.build_furniture()
    art.build_fx()
    art.build_wallsegs()
    art.FLOORPLAN.update(art.floorplan_json())
    os.makedirs(OUT, exist_ok=True)
    for name, kind, theme, night, state, visible, kanban, safe, balls, pizza in CASES:
        cubicle = art.THEMES[theme]["cubicle"]
        img, _ = art.render_scene(
            kind,
            theme,
            [],
            state=state,
            visible=visible,
            tints={f"dev_part_{i}": cubicle for i in range(6)},
            night=night,
            kanban=kanban,
            safe=safe,
            balls=balls,
            pizza=pizza,
        )
        path = os.path.join(OUT, f"{name}.png")
        img.save(path)
        print(path)


if __name__ == "__main__":
    main()
