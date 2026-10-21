// @vitest-environment jsdom
import { expect, test, vi } from "vite-plus/test";
import phrases from "../design/handoff/phrases.ko.json";
import { showScale, type Invoke } from "./scale.ts";

const settle = () => new Promise((r) => setTimeout(r, 0));

function mount(invoke: ReturnType<typeof vi.fn>) {
  const root = document.createElement("div");
  const done = showScale(root, invoke as unknown as Invoke);
  const cards = [...root.querySelectorAll<HTMLButtonElement>("button.sc__opt")];
  return { root, done, cards };
}

test("고르기 전에는 끝나지 않고 set_settings 도 부르지 않는다", async () => {
  const invoke = vi.fn().mockResolvedValue(undefined);
  const { done } = mount(invoke);
  const spy = vi.fn();
  void done.then(spy);
  await settle();
  expect(spy).not.toHaveBeenCalled();
  expect(invoke).not.toHaveBeenCalled();
});

test("카드를 누르면 set_settings 를 한 번 부르고 그 규모로 풀린다. 두 카드 모두 잠긴다. 빈 이름은 null", async () => {
  const invoke = vi.fn().mockResolvedValue(undefined);
  const { done, cards } = mount(invoke);
  cards[1].click();
  expect(cards.map((c) => c.disabled)).toEqual([true, true]);
  expect(await done).toEqual({ scale: "Small", companyName: null });
  expect(invoke).toHaveBeenCalledTimes(1);
  expect(invoke).toHaveBeenCalledWith("set_settings", {
    dto: { scale: "Small", companyName: null },
  });
});

test("이름을 넣고 규모를 고르면 둘을 한 번에 보낸다. 앞뒤 공백은 뗀다", async () => {
  const invoke = vi.fn().mockResolvedValue(undefined);
  const { root, done, cards } = mount(invoke);
  const input = root.querySelector<HTMLInputElement>(".sc__company input")!;
  input.value = "  야근상사 ";
  cards[0].click();
  expect(await done).toEqual({ scale: "Large", companyName: "야근상사" });
  expect(invoke).toHaveBeenCalledTimes(1);
  expect(invoke).toHaveBeenCalledWith("set_settings", {
    dto: { scale: "Large", companyName: "야근상사" },
  });
});

test("이름 칸은 라벨·자리표시·20자 상한을 갖고 처음 초점을 받는다", () => {
  document.body.replaceChildren();
  const root = document.body.appendChild(document.createElement("div"));
  void showScale(root, vi.fn() as unknown as Invoke);
  const input = root.querySelector<HTMLInputElement>(".sc__company input")!;
  expect(root.querySelector(".sc__company")?.textContent).toBe(phrases.onboarding.companyLabel);
  expect(input.placeholder).toBe(phrases.app.brand);
  expect(input.maxLength).toBe(20);
  expect(document.activeElement).toBe(input);
});

test("저장이 실패해도 멈추지 않고 진행한다", async () => {
  vi.spyOn(console, "error").mockImplementation(() => {});
  const invoke = vi.fn().mockRejectedValue("저장 실패");
  const { done, cards } = mount(invoke);
  cards[0].click();
  expect((await done).scale).toBe("Large");
});

test("문구는 phrases 와 같다", () => {
  const { root, cards } = mount(vi.fn());
  expect(root.querySelector("h1")?.textContent).toBe(phrases.onboarding.title);
  expect(root.querySelector(".sc__brand")?.textContent).toBe(
    phrases.app.brand + phrases.app.brandSub,
  );
  expect(cards.map((c) => c.querySelector(".sc__name")?.textContent)).toEqual([
    phrases.onboarding.large.title,
    phrases.onboarding.small.title,
  ]);
});
