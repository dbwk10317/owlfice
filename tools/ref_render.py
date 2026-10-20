# /// script
# requires-python = ">=3.11"
# dependencies = ["pillow"]
# ///
"""T18 렌더러 검수용 참조 그림. 직원 없이 기본 상태로 그린 평면도 두 장을 dev/ref/ 에 쓴다.

호출: uv run tools/ref_render.py
정답 그리기는 design/tools/owlfice_art.py 의 render_scene 이다. 그 파일은 고치지 않고 import 만 한다.
dev/ref.ts 가 같은 상태를 앱 렌더러로 그려 이 PNG 와 픽셀 비교한다 (HANDOFF 6.7).
"""

import os
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
# design/ 에 __pycache__ 를 남기지 않는다 (design/ 은 읽기만)
sys.dont_write_bytecode = True
sys.path.insert(0, os.path.join(ROOT, "design", "tools"))

import owlfice_art as art

OUT = os.path.join(ROOT, "dev", "ref")

# dev/ref.ts 의 CASES 와 같은 이름·상태여야 한다
CASES = [("large", "claude"), ("small", "codex")]


def main():
    # render_scene 이 읽는 OFFICE·PROPS·FLOORPLAN 을 채운다. 시트 파일은 쓰지 않는 함수들만 부른다
    art.build_tiles()
    art.build_shell()
    art.build_wall_items()
    art.build_furniture()
    art.build_fx()
    art.build_wallsegs()
    art.FLOORPLAN.update(art.floorplan_json())
    os.makedirs(OUT, exist_ok=True)
    for kind, theme in CASES:
        cubicle = art.THEMES[theme]["cubicle"]
        # 빈 사무실이라 파티션은 모두 테마 색 (HANDOFF 6.5). 전광판 숫자는 소품 상태(T23)라 그리지 않는다
        img, _ = art.render_scene(
            kind,
            theme,
            [],
            tints={f"dev_part_{i}": cubicle for i in range(6)},
            safe="",
        )
        path = os.path.join(OUT, f"{kind}-{theme}.png")
        img.save(path)
        print(path)


if __name__ == "__main__":
    main()
