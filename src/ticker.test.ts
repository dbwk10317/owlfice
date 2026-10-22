// @vitest-environment jsdom
import { expect, test, vi } from "vite-plus/test";
import { createEventCollector, createTicker, TICKER_MAX } from "./ticker.ts";
import { act, emp, iso, office, team } from "./test-office.ts";

const withRecent = (seq: number, officeId: string, recent: ReturnType<typeof act>[]) =>
  office(officeId, seq, [team("t", [emp("boss"), emp("a", { name: "서준", recent })])]);

test("새 행적만 시간순으로 모으고 같은 스냅샷은 다시 세지 않는다", () => {
  const c = createEventCollector();
  const late = act({ sentence: "나중", since: iso(14, 31) });
  const early = act({ sentence: "먼저", since: iso(14, 29) });
  expect(c.ingest(withRecent(1, "o", [late, early]), false)).toBe(true);
  expect(c.events().map((e) => e.text)).toEqual(["서준 대리, 먼저", "서준 대리, 나중"]);
  expect(c.events().map((e) => e.time)).toEqual(["14:29", "14:31"]);
  expect(c.ingest(withRecent(2, "o", [late, early]), false)).toBe(false);
  const newest = act({ sentence: "최신", since: iso(14, 33) });
  expect(c.ingest(withRecent(3, "o", [newest, late, early]), false)).toBe(true);
  expect(c.events()[c.events().length - 1]?.text).toBe("서준 대리, 최신");
});

test("중소기업은 앞에 팀 이름, 사무실이 바뀌면 비운다, 상한이 있다", () => {
  const c = createEventCollector();
  c.ingest(withRecent(1, "o", [act({ sentence: "일", since: iso(14, 1) })]), true);
  expect(c.events()[0].text).toBe("[이름boss 부장 팀] 서준 대리, 일");
  c.ingest(withRecent(1, "p", []), false);
  expect(c.events()).toHaveLength(0);
  const many = Array.from({ length: TICKER_MAX + 5 }, (_, i) =>
    act({ sentence: `s${i}`, since: iso(14, 0, i) }),
  );
  c.ingest(withRecent(2, "p", many), false);
  expect(c.events()).toHaveLength(TICKER_MAX);
  expect(c.events()[c.events().length - 1]?.text).toBe(`서준 대리, s${TICKER_MAX + 4}`);
});

test("두 팀에 같은 agentId 가 있어도 사건은 둘 다 남고 teamId 가 맞다", () => {
  const c = createEventCollector();
  const same = [act({ sentence: "일", since: iso(14, 1) })];
  const o = office("o", 1, [
    team("t1", [emp("boss"), emp("a", { name: "서준", recent: same })]),
    team("t2", [emp("boss"), emp("a", { name: "서준", recent: same })]),
  ]);
  c.ingest(o, false);
  expect(c.events().map((e) => e.teamId)).toEqual(["t1", "t2"]);
  expect(c.ingest(o, false)).toBe(false);
});

test("티커 DOM: 비면 조용한 밤, 있으면 두 번 이어 붙이고 클릭은 팀·직원 id 를 넘긴다", () => {
  const pick = vi.fn();
  const t = createTicker(document, pick);
  expect(t.el.querySelector(".ow-ticker__quiet")?.textContent).toBe("조용한 밤입니다");
  const c = createEventCollector();
  c.ingest(withRecent(1, "o", [act({ since: iso(14, 1) })]), false);
  t.setEvents(c.events());
  const items = t.el.querySelectorAll<HTMLElement>(".ow-ticker__item");
  expect(items).toHaveLength(2);
  expect(t.el.textContent).not.toContain("조용한 밤입니다");
  items[0].click();
  expect(pick).toHaveBeenCalledWith("t", "a");
  t.setEvents([]);
  expect(t.el.querySelector(".ow-ticker__quiet")?.textContent).toBe("조용한 밤입니다");
});

test("티커 항목은 시각·팀·이름·요약을 나누고 호버와 초점에서 흐름을 멈춘다", () => {
  const c = createEventCollector();
  c.ingest(withRecent(1, "o", [act({ sentence: "빌드 완료", since: iso(14, 1) })]), true);
  const t = createTicker(document, vi.fn());
  document.body.append(t.el);
  t.setEvents(c.events());
  const track = t.el.querySelector<HTMLElement>(".ow-ticker__track")!;
  const first = t.el.querySelector<HTMLElement>(".ow-ticker__item")!;
  expect(first.querySelector(".ow-ticker__time")?.textContent).toBe("14:01");
  expect(first.querySelector(".ow-ticker__team")?.textContent).toBe("[이름boss 부장 팀]");
  expect(first.querySelector(".ow-ticker__who")?.textContent).toBe("서준 대리, ");
  expect(first.querySelector(".ow-ticker__summary")?.textContent).toBe("빌드 완료");
  expect(track.classList.contains("is-flowing")).toBe(true);
  first.dispatchEvent(new MouseEvent("mouseenter"));
  expect(track.classList.contains("is-paused")).toBe(true);
  first.focus();
  first.dispatchEvent(new MouseEvent("mouseleave"));
  expect(track.classList.contains("is-paused")).toBe(true);
  first.blur();
  expect(track.classList.contains("is-paused")).toBe(false);
  t.el.remove();
});

test("움직임 줄이기는 최신 항목 하나를 정지 표시한다", () => {
  vi.stubGlobal("matchMedia", () => ({
    matches: true,
    addEventListener() {},
    removeEventListener() {},
  }));
  try {
    const c = createEventCollector();
    c.ingest(
      withRecent(1, "o", [
        act({ sentence: "예전", since: iso(14, 1) }),
        act({ sentence: "최신", since: iso(14, 2) }),
      ]),
      false,
    );
    const t = createTicker(document, vi.fn());
    t.setEvents(c.events());
    expect(t.el.querySelectorAll(".ow-ticker__item")).toHaveLength(1);
    expect(t.el.querySelector(".ow-ticker__summary")?.textContent).toBe("최신");
    expect(t.el.querySelector(".ow-ticker__track")?.classList.contains("is-flowing")).toBe(false);
  } finally {
    vi.unstubAllGlobals();
  }
});

test("숨은 동안 폭이 0이었다가 보이면 흐름 길이를 다시 잰다", () => {
  const t = createTicker(document, vi.fn());
  document.body.append(t.el);
  const width = vi.spyOn(HTMLElement.prototype, "scrollWidth", "get");
  width.mockReturnValue(0);
  const c = createEventCollector();
  c.ingest(withRecent(1, "o", [act({ since: iso(14, 1) })]), false);
  t.setEvents(c.events());
  const track = t.el.querySelector<HTMLElement>(".ow-ticker__track")!;
  expect(track.style.animationDuration).toBe("");
  width.mockReturnValue(800);
  t.remeasure();
  expect(track.style.animationDuration).toBe("10s");
  width.mockRestore();
  t.el.remove();
});

test("실행 중 움직임 설정은 마지막 사건 목록을 즉시 다시 그리고 같은 목록 전달에도 반영한다", () => {
  const motion = Object.assign(new EventTarget(), { matches: false });
  vi.stubGlobal("matchMedia", () => motion);
  const c = createEventCollector();
  c.ingest(
    withRecent(
      1,
      "o",
      Array.from({ length: 5 }, (_, i) => act({ sentence: `사건${i}`, since: iso(14, i) })),
    ),
    false,
  );
  const pick = vi.fn();
  const t = createTicker(document, pick);
  const items = () => t.el.querySelectorAll<HTMLElement>(".ow-ticker__item");
  const track = t.el.querySelector(".ow-ticker__track")!;
  try {
    t.setEvents(c.events());
    expect(items()).toHaveLength(10);
    motion.matches = true;
    motion.dispatchEvent(new Event("change"));
    expect(items()).toHaveLength(1);
    expect(items()[0].querySelector(".ow-ticker__summary")?.textContent).toBe("사건4");
    expect(track.classList.contains("is-flowing")).toBe(false);
    items()[0].click();
    expect(pick).toHaveBeenCalledWith("t", "a");
    motion.matches = false;
    motion.dispatchEvent(new Event("change"));
    expect(items()).toHaveLength(10);
    expect(track.classList.contains("is-flowing")).toBe(true);
    motion.matches = true;
    t.setEvents(c.events());
    expect(items()).toHaveLength(1);
    motion.matches = false;
    t.setEvents(c.events());
    expect(items()).toHaveLength(10);
    t.setEvents([]);
    motion.matches = true;
    motion.dispatchEvent(new Event("change"));
    expect(items()).toHaveLength(0);
    expect(t.el.querySelector(".ow-ticker__quiet")?.textContent).toBe("조용한 밤입니다");
    expect(track.classList.contains("is-flowing")).toBe(false);
    motion.matches = false;
    t.setEvents(c.events());
    t.destroy();
    motion.matches = true;
    motion.dispatchEvent(new Event("change"));
    expect(items()).toHaveLength(10);
  } finally {
    t.destroy();
    vi.unstubAllGlobals();
  }
});
