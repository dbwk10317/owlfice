import { expect, test } from "vite-plus/test";
import { canvasScale, panLimits, toCss } from "./renderer.ts";

test("배율은 장치 픽셀 기준 정수, CSS 크기는 백킹 / DPR", () => {
  // 2× 는 1040×640 장치 픽셀이 필요하다 (PLAN 3.2)
  expect(canvasScale(1040, 664, 1)).toEqual({
    scale: 2,
    backingW: 1040,
    backingH: 640,
    cssW: 1040,
    cssH: 640,
  });
  expect(canvasScale(1039, 664, 1).scale).toBe(1);
  expect(canvasScale(1560, 960, 1).scale).toBe(3);
  // 125%: 장치 1040 이면 2×, CSS 크기는 소수 없이 832
  expect(canvasScale(1040, 640, 1.25)).toEqual({
    scale: 2,
    backingW: 1040,
    backingH: 640,
    cssW: 832,
    cssH: 512,
  });
  // 경계: 실제 장치 픽셀이 1039 면 CSS 가 반올림돼 831.2→831 이어도 배율이 안 올라간다
  expect(canvasScale(1039, 640, 1.25).scale).toBe(1);
  // 150%: 1560×960 이면 3×, 그 밑이면 2×
  expect(canvasScale(1560, 960, 1.5)).toMatchObject({ scale: 3, cssW: 1040, cssH: 640 });
  expect(canvasScale(1559, 960, 1.5).scale).toBe(2);
  // 무대가 1× 보다 작아도 1 밑으로 내려가지 않는다 (패닝이 맡는다)
  expect(canvasScale(300, 200, 1)).toEqual({
    scale: 1,
    backingW: 520,
    backingH: 320,
    cssW: 520,
    cssH: 320,
  });
});

test("패닝은 축마다 캔버스가 무대보다 클 때만 열린다", () => {
  // 넘치지 않으면 어느 창 크기에서도 닫힘
  expect(panLimits(520, 320, 520, 320, 1)).toEqual({ maxX: 0, maxY: 0 });
  expect(panLimits(1040, 640, 1600, 900, 1)).toEqual({ maxX: 0, maxY: 0 });
  // 가로만 넘침: 반씩
  expect(panLimits(520, 320, 400, 400, 1)).toEqual({ maxX: 60, maxY: 0 });
  // 세로만 넘침, 큰 창이어도
  expect(panLimits(520, 320, 1000, 300, 1)).toEqual({ maxX: 0, maxY: 10 });
  // 장치 픽셀 차이는 CSS px 로
  expect(panLimits(520, 320, 400, 320, 2)).toEqual({ maxX: 30, maxY: 0 });
});

test("DOM 오버레이 좌표 = 1× 좌표 × scale / dpr", () => {
  expect(toCss(260, 180, 1, 1)).toEqual({ left: 260, top: 180 });
  expect(toCss(260, 180, 2, 1)).toEqual({ left: 520, top: 360 });
  expect(toCss(260, 180, 3, 1.5)).toEqual({ left: 520, top: 360 });
  expect(toCss(100, 50, 1, 1.25)).toEqual({ left: 80, top: 40 });
});
