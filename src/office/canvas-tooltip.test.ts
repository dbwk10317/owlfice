// @vitest-environment jsdom
import { expect, test } from "vite-plus/test";
import { act, emp, office, team } from "../test-office.ts";
import { CanvasTooltip, propTooltip, roomTooltip } from "./canvas-tooltip.ts";

test("통계 전문은 표시 상한 없이 나오고 시계는 가장 오래된 팀을 쓴다", () => {
  const a = team("a", [], { started: "2026-01-01T00:00:00Z" });
  const b = team("b", [], { started: "2026-01-01T02:00:00Z" });
  const o = office("o", 1, [b, a]);
  o.shared.stats = { commits: 12345, filesEdited: 10, commands: 999999, turns: 0 };
  expect(propTooltip("stats_board", o, 0)).toContain("실행 명령 999999");
  expect(propTooltip("clock", o, Date.parse("2026-01-01T03:00:00Z"))).toContain("경과 3시간");
});

test("근태판은 퇴근 팀·퇴근 직원·외근·외출을 자석과 같은 규칙으로 표시한다", () => {
  const a = team("a", [
    emp("a"),
    emp("b", { activity: act({ room: "Outside" }) }),
    emp("c", { retiredAt: "2026-01-01T00:00:00Z" }),
    emp("d", { departedAt: "2026-01-01T00:00:00Z" }),
  ]);
  const b = team("b", [emp("e")], { phase: "Off" });
  const text = propTooltip("attendance", office("o", 1, [a, b]), 0);
  expect(text).toContain("이름a · 출근");
  expect(text).toContain("이름b · 외근");
  expect(text).toContain("이름c · 외출");
  expect(text).toContain("이름d · 퇴근");
  expect(text).toContain("이름e · 퇴근");
});

test("출입구의 문·출고장 사건을 합쳐 최신10개만 표시하고 추정을 보존한다", () => {
  const o = office("o", 1, [team("t", [emp("e", { name: "홍대리" })])]);
  o.roomRecent = ["Door", "Shipping"].map((room, group) => ({
    room: room as "Door" | "Shipping",
    events: Array.from({ length: 10 }, (_, i) => ({
      teamId: "t",
      agentId: "e",
      activity: act({
        sentence: `사건${group * 10 + i}`,
        since: new Date((group * 10 + i) * 60_000).toISOString(),
        confidence: "Inferred",
      }),
    })),
  }));
  const text = roomTooltip("entrance", "출입구", o, 0);
  expect(text.split("\n")).toHaveLength(11);
  expect(text.split("\n")[1]).toContain("홍대리 · 사건19 (추정)");
  expect(text).not.toContain("사건0");
  expect(roomTooltip("build", "빌드실", o, 0)).toContain("최근 사건이 없습니다.");
});

test("원문 HTML을 실행하지 않으며 Escape·바깥 클릭으로 닫고 화면 안에 둔다", () => {
  const host = document.createElement("div");
  const source = document.createElement("button");
  document.body.append(host, source);
  const tip = new CanvasTooltip(host);
  tip.el.getBoundingClientRect = () => ({ width: 300, height: 200 }) as DOMRect;
  tip.show("<img src=x onerror=alert(1)>\n끝", 100000, 100000, source);
  expect(tip.el.querySelector("img")).toBeNull();
  expect(tip.el.textContent).toContain("<img");
  expect(Number.parseFloat(tip.el.style.left) + 300).toBeLessThanOrEqual(window.innerWidth);
  expect(Number.parseFloat(tip.el.style.top) + 200).toBeLessThanOrEqual(window.innerHeight);
  expect(source.getAttribute("aria-describedby")).toBe(tip.el.id);
  document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
  expect(tip.el.hidden).toBe(true);
  expect(source.hasAttribute("aria-describedby")).toBe(false);
  tip.show("긴 전문", 1, 1);
  tip.el.dispatchEvent(new Event("pointerdown", { bubbles: true }));
  expect(tip.el.hidden).toBe(false);
  document.body.dispatchEvent(new Event("pointerdown", { bubbles: true }));
  expect(tip.el.hidden).toBe(true);
  host.remove();
  source.remove();
});
