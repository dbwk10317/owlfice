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

test("티커 DOM: 비면 조용한 밤, 있으면 두 번 이어 붙이고 클릭은 직원 id 를 넘긴다", () => {
  const pick = vi.fn();
  const t = createTicker(document, pick);
  expect(t.el.textContent).toContain("조용한 밤입니다");
  const c = createEventCollector();
  c.ingest(withRecent(1, "o", [act({ since: iso(14, 1) })]), false);
  t.setEvents(c.events());
  const items = t.el.querySelectorAll<HTMLElement>(".ow-ticker__item");
  expect(items).toHaveLength(2);
  expect(t.el.textContent).not.toContain("조용한 밤입니다");
  items[0].click();
  expect(pick).toHaveBeenCalledWith("a");
  t.setEvents([]);
  expect(t.el.textContent).toContain("조용한 밤입니다");
});
