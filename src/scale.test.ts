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

test("이름 칸은 라벨·자리표시를 갖고 처음 초점을 받으며 UTF-16 제한을 두지 않는다", () => {
  document.body.replaceChildren();
  const root = document.body.appendChild(document.createElement("div"));
  void showScale(root, vi.fn() as unknown as Invoke);
  const input = root.querySelector<HTMLInputElement>(".sc__company input")!;
  expect(root.querySelector(".sc__company")?.textContent).toBe(phrases.onboarding.companyLabel);
  expect(input.placeholder).toBe(phrases.app.brand);
  expect(input.maxLength).toBe(-1);
  expect(document.activeElement).toBe(input);
});

test("저장 실패 뒤 규모가 반영됐으면 오류를 보이고 명시적으로 계속한다", async () => {
  const invoke = vi
    .fn()
    .mockRejectedValueOnce("저장 실패")
    .mockResolvedValueOnce({ scale: "Large", companyName: "확인한 회사" });
  const { root, done, cards } = mount(invoke);
  const resolved = vi.fn();
  void done.then(resolved);
  cards[0].click();
  await settle();
  expect(invoke.mock.calls.map(([cmd]) => cmd)).toEqual(["set_settings", "get_settings"]);
  expect(root.querySelector("[role=alert]")?.textContent).toContain("저장하지 못했습니다");
  const resume = root.querySelector<HTMLButtonElement>(".sc__continue")!;
  expect(resume.hidden).toBe(false);
  expect(resolved).not.toHaveBeenCalled();
  resume.click();
  expect(await done).toEqual({ scale: "Large", companyName: "확인한 회사" });
});

test("저장 실패 뒤 규모가 미반영이면 카드에서 다시 선택해 성공할 수 있다", async () => {
  const invoke = vi
    .fn()
    .mockRejectedValueOnce("저장 실패")
    .mockResolvedValueOnce({ scale: null, companyName: null })
    .mockResolvedValueOnce(undefined);
  const { root, done, cards } = mount(invoke);
  cards[1].click();
  await settle();
  expect(cards.map((c) => c.disabled)).toEqual([false, false]);
  expect(root.querySelector<HTMLButtonElement>(".sc__continue")?.hidden).toBe(true);
  expect(root.querySelector("[role=alert]")?.textContent).toContain("다시 선택");
  cards[1].click();
  expect(await done).toEqual({ scale: "Small", companyName: null });
});

test("재조회도 실패하면 카드 재시도만 허용한다", async () => {
  const invoke = vi
    .fn()
    .mockRejectedValueOnce("저장 실패")
    .mockRejectedValueOnce("조회 실패")
    .mockResolvedValueOnce(undefined);
  const { root, done, cards } = mount(invoke);
  cards[0].click();
  await settle();
  expect(cards.map((c) => c.disabled)).toEqual([false, false]);
  expect(root.querySelector<HTMLButtonElement>(".sc__continue")?.hidden).toBe(true);
  expect(root.querySelector("[role=alert]")?.textContent).toContain("확인하지 못했습니다");
  cards[0].click();
  expect((await done).scale).toBe("Large");
});

test("이름은 Unicode 20자까지 보내고 21자는 안내 후 저장하지 않는다", async () => {
  const invoke = vi.fn().mockResolvedValue(undefined);
  const { root, done, cards } = mount(invoke);
  const input = root.querySelector<HTMLInputElement>(".sc__company input")!;
  input.value = "🐈".repeat(21);
  cards[0].click();
  await settle();
  expect(invoke).not.toHaveBeenCalled();
  expect(root.querySelector("[role=alert]")?.textContent).toContain("20자");
  expect(cards.map((c) => c.disabled)).toEqual([false, false]);
  input.value = ` ${"🐈".repeat(20)} `;
  cards[0].click();
  expect(await done).toEqual({ scale: "Large", companyName: "🐈".repeat(20) });
  expect(invoke).toHaveBeenCalledTimes(1);
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
