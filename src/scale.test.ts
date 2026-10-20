import { expect, test, vi } from "vite-plus/test";
import { createPicker, keyChoice, moveFocus } from "./scale.ts";

const settle = () => new Promise((r) => setTimeout(r, 0));

test("고르기 전에는 끝나지 않고 set_settings 도 부르지 않는다", async () => {
  const invoke = vi.fn().mockResolvedValue(undefined);
  const p = createPicker(invoke);
  const spy = vi.fn();
  void p.done.then(spy);
  await settle();
  expect(spy).not.toHaveBeenCalled();
  expect(invoke).not.toHaveBeenCalled();
});

test("고르면 set_settings 로 저장한 뒤 규모로 풀린다. 두 번째 선택은 무시한다", async () => {
  const invoke = vi.fn().mockResolvedValue(undefined);
  const p = createPicker(invoke);
  await p.choose("Small");
  await p.choose("Large");
  expect(await p.done).toBe("Small");
  expect(invoke).toHaveBeenCalledTimes(1);
  expect(invoke).toHaveBeenCalledWith("set_settings", { dto: { scale: "Small" } });
});

test("저장이 실패해도 멈추지 않고 진행한다", async () => {
  vi.spyOn(console, "error").mockImplementation(() => {});
  const invoke = vi.fn().mockRejectedValue("저장 실패");
  const p = createPicker(invoke);
  await p.choose("Large");
  expect(await p.done).toBe("Large");
});

test("키보드: 숫자로 고르고 방향키로 카드 사이를 옮긴다", () => {
  expect([keyChoice("1"), keyChoice("2"), keyChoice("3"), keyChoice("a")]).toEqual([
    "Large",
    "Small",
    null,
    null,
  ]);
  expect(moveFocus(0, "ArrowRight")).toBe(1);
  expect(moveFocus(1, "ArrowRight")).toBe(1);
  expect(moveFocus(1, "ArrowLeft")).toBe(0);
  expect(moveFocus(0, "ArrowLeft")).toBe(0);
  expect(moveFocus(1, "Tab")).toBe(1);
});
