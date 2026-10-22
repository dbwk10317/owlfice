// @vitest-environment jsdom
import { expect, test } from "vite-plus/test";
import { compare } from "./ref.ts";

test("반투명 소품 마스크의 오차 2만 허용하고 오차 3이나 바깥 불투명 오차 2는 남긴다", () => {
  const ref = new Uint8ClampedArray([10, 20, 30, 255, 10, 20, 30, 255, 10, 20, 30, 255]);
  const got = new Uint8ClampedArray([10, 20, 32, 255, 10, 20, 33, 255, 10, 20, 32, 255]);
  const { count, diff } = compare(ref, got, 1, undefined, (x, y) => y === 0 && x < 2);
  expect(count).toBe(2);
  expect(Array.from(diff.slice(0, 4))).toEqual([10, 20, 30, 64]);
  expect(Array.from(diff.slice(4))).toEqual([255, 0, 0, 255, 255, 0, 0, 255]);
  expect(compare(ref, got, 1).count).toBe(3);
});
