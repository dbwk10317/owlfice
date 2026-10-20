import { expect, test } from "vite-plus/test";
import { canvasScale, toCss, viewMode } from "./renderer.ts";

test("배율은 장치 픽셀 기준 정수, CSS 크기는 백킹 / DPR", () => {
  // 2× 는 1040×640 이 필요하다 (PLAN 3.2). 1600×720 창의 무대가 1040×664
  expect(canvasScale(1040, 664, 1)).toEqual({
    scale: 2,
    backingW: 1040,
    backingH: 640,
    cssW: 1040,
    cssH: 640,
  });
  expect(canvasScale(1039, 664, 1).scale).toBe(1);
  expect(canvasScale(1560, 960, 1).scale).toBe(3);
  // 125%: 같은 CSS 크기에서 더 큰 배율, CSS 크기는 소수
  expect(canvasScale(832, 512, 1.25)).toEqual({
    scale: 2,
    backingW: 1040,
    backingH: 640,
    cssW: 832,
    cssH: 512,
  });
  expect(canvasScale(831, 512, 1.25).scale).toBe(1);
  // 150%: 1100 창의 무대(540×584)도 장치 픽셀로는 810 이라 1×
  const s = canvasScale(540, 584, 1.5);
  expect(s.scale).toBe(1);
  expect(s.cssW).toBeCloseTo(346.667, 3);
  expect(s.cssH).toBeCloseTo(213.333, 3);
  expect(canvasScale(1040, 664, 1.5)).toEqual({
    scale: 3,
    backingW: 1560,
    backingH: 960,
    cssW: 1040,
    cssH: 640,
  });
  // 무대가 1× 보다 작아도 1 밑으로 내려가지 않는다 (패닝이 맡는다)
  expect(canvasScale(300, 200, 1)).toEqual({
    scale: 1,
    backingW: 520,
    backingH: 320,
    cssW: 520,
    cssH: 320,
  });
});

test("창 크기 구간: 너비 720 미만 가로 패닝, 높이 480 미만 세로 패닝", () => {
  expect(viewMode(1600, 960)).toEqual({ panX: false, panY: false });
  expect(viewMode(720, 480)).toEqual({ panX: false, panY: false });
  expect(viewMode(719, 640)).toEqual({ panX: true, panY: false });
  expect(viewMode(800, 479)).toEqual({ panX: false, panY: true });
  expect(viewMode(480, 360)).toEqual({ panX: true, panY: true });
});

test("DOM 오버레이 좌표 = 1× 좌표 × scale / dpr", () => {
  expect(toCss(260, 180, 1, 1)).toEqual({ left: 260, top: 180 });
  expect(toCss(260, 180, 2, 1)).toEqual({ left: 520, top: 360 });
  expect(toCss(260, 180, 3, 1.5)).toEqual({ left: 520, top: 360 });
  expect(toCss(100, 50, 1, 1.25)).toEqual({ left: 80, top: 40 });
});
