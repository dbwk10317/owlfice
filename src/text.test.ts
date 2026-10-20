import { expect, test } from "vite-plus/test";
import { durationText, fill, fillElapsed, hhmm } from "./text.ts";

test("fill: 없는 자리는 빈 글자", () => {
  expect(fill("{a} {b}!", { a: 1 })).toBe("1 !");
});

test("durationText 초 표기: 경계와 정각", () => {
  expect(durationText(59, true)).toBe("59초");
  expect(durationText(60, true)).toBe("1분");
  expect(durationText(64, true)).toBe("1분 4초");
  expect(durationText(3599, true)).toBe("59분 59초");
  expect(durationText(3600, true)).toBe("1시간");
  expect(durationText(3900, true)).toBe("1시간 5분");
  expect(durationText(13 * 3600 + 20 * 60, true)).toBe("13시간 20분");
});

test("durationText 분 표기: 초는 버린다", () => {
  expect(durationText(60, false)).toBe("1분");
  expect(durationText(3599, false)).toBe("59분");
  expect(durationText(3600, false)).toBe("1시간");
  expect(durationText(13 * 3600, false)).toBe("13시간");
  expect(durationText(13 * 3600 + 20 * 60, false)).toBe("13시간 20분");
});

test("durationText: 음수·NaN 은 0", () => {
  expect(durationText(-5, true)).toBe("0초");
  expect(durationText(Number.NaN, true)).toBe("0초");
  expect(durationText(-5, false)).toBe("0분");
});

test("fillElapsed: 플래그가 false 면 그대로, 백틱 안은 그대로", () => {
  expect(fillElapsed("{elapsed}째", 12, false)).toBe("{elapsed}째");
  expect(fillElapsed("· {elapsed}째", 12, true)).toBe("· 12초째");
  expect(fillElapsed("`echo {elapsed}` · {elapsed}", 1, true)).toBe("`echo {elapsed}` · 1초");
  expect(fillElapsed("그대로", 5, true)).toBe("그대로");
  expect(fillElapsed("a {elapsed} b", null, true)).toBe("a  b");
});

test("hhmm: 두 자리로 맞춘다", () => {
  expect(hhmm(new Date(2026, 0, 2, 3, 4).getTime())).toBe("03:04");
});
