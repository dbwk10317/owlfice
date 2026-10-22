// @vitest-environment jsdom
import { afterEach, beforeEach, expect, test, vi } from "vite-plus/test";
import { layoutOf, mountHeader } from "./header.ts";
import { mountPanel, type Viewport } from "./sidebar-right.ts";
import { createStore } from "./store.ts";
import { emp, office, team } from "./test-office.ts";
import type { OfficeDto, OfficeListDto, OfficeSummaryDto, Scale } from "./types/ipc.ts";

// 2026-01-01 22:47 (로컬). 팀 시작은 12분 전
const NOW = new Date(2026, 0, 1, 22, 47, 0);
const STARTED = new Date(NOW.getTime() - 12 * 60_000).toISOString();

beforeEach(() => {
  vi.useFakeTimers();
  vi.setSystemTime(NOW);
});
afterEach(() => {
  vi.useRealTimers();
  document.body.innerHTML = "";
});

const summary = (over: Partial<OfficeSummaryDto> = {}): OfficeSummaryDto => ({
  officeId: "o",
  building: "B",
  floor: 2,
  room: 203,
  label: "B동 203호",
  phase: "Working",
  dot: "Working",
  asking: false,
  border: "Plain",
  delayed: false,
  teams: [
    {
      teamId: "t",
      bossName: "김하늘",
      title: "t",
      folder: "f",
      others: 2,
      started: STARTED,
      lastActivity: STARTED,
      readFailed: false,
      phase: "Working",
      dot: "Working",
      asking: false,
      askingCount: 0,
      border: "Plain",
      delayed: false,
      hostBadge: "WSL",
      workType: "BugFix",
    },
  ],
  ...over,
});

function setup(
  o: OfficeSummaryDto,
  scale: Scale = "Large",
  over: Partial<OfficeDto> = {},
  bandSlot?: () => HTMLElement | null,
) {
  const store = createStore<OfficeListDto, OfficeDto>();
  const sel = {
    ...office(o.officeId, 1, [team("t", [emp("a")], { title: "로그인 버그 고치기" })]),
    ...over,
  };
  store.applyWorld({
    listSeq: 1,
    rebuilt: false,
    scale,
    offices: [o],
    selectedId: o.officeId,
    selectRequest: 0,
    selected: sel,
  });
  let size = { w: 1280, h: 800 };
  const cbs = new Set<() => void>();
  const vp: Viewport & { set(w: number): void } = {
    size: () => size,
    subscribe: (cb) => (cbs.add(cb), () => cbs.delete(cb)),
    set(w) {
      size = { w, h: 800 };
      cbs.forEach((cb) => cb());
    },
  };
  const app = document.createElement("div");
  const header = document.createElement("header");
  const panelHost = document.createElement("div");
  app.append(header, panelHost);
  document.body.append(app);
  const setRoomLabels = vi.fn();
  const setScale = vi.fn();
  const centerCanvas = vi.fn();
  const toggleMaximize = vi.fn().mockResolvedValue(undefined);
  const stage = document.createElement("div");
  app.append(stage);
  let handle: ReturnType<typeof mountHeader> | null = null;
  const panel = mountPanel(panelHost, store, {
    viewport: vp,
    onDrawerChange: (open) => handle?.setDrawerOpen(open),
  });
  handle = mountHeader(app, header, store, {
    viewport: vp,
    bandSlot,
    togglePanel: () => panel.toggleDrawer(),
    setRoomLabels,
    setScale,
    stage,
    centerCanvas,
    toggleMaximize,
  });
  const q = (sel: string) => header.querySelector<HTMLElement>(sel)!;
  return {
    store,
    app,
    header,
    vp,
    setRoomLabels,
    setScale,
    centerCanvas,
    toggleMaximize,
    stage,
    handle,
    q,
    panelHost,
  };
}

test("3.2 표의 너비 경계", () => {
  expect([1600, 1100, 1099, 900, 899, 720, 719].map(layoutOf)).toEqual([
    "wide",
    "wide",
    "drawer",
    "drawer",
    "rail",
    "rail",
    "select",
  ]);
});

test("설정 갱신으로 본사 이름과 소리 토글이 바로 바뀌고 실패는 알린다", async () => {
  const s = setup(summary());
  s.store.applyList({ ...s.store.state.list!, listSeq: 2, selectedId: null });
  s.handle!.setSettings("새 회사", true);
  expect(s.q(".ow-header__title").textContent).toContain("새 회사");
  expect(s.q('[aria-label="소리 끄기"]').getAttribute("aria-pressed")).toBe("true");

  const app = document.createElement("div");
  const header = document.createElement("header");
  app.append(header);
  document.body.append(app);
  const onSound = vi.fn().mockRejectedValue(new Error("저장 실패"));
  mountHeader(app, header, s.store, { togglePanel() {}, setRoomLabels() {}, onSound });
  const toggle = header.querySelector<HTMLButtonElement>('[aria-label="소리 켜기"]')!;
  toggle.click();
  expect(toggle.disabled).toBe(true);
  await Promise.resolve();
  await Promise.resolve();
  expect(onSound).toHaveBeenCalledExactlyOnceWith(true);
  expect(app.querySelector('[role="alert"]')!.textContent).toContain("저장 실패");
  expect(toggle.disabled).toBe(false);
});

test("대기업: 이름, 제품·호스트, 세션 제목, 근무 중 경과", () => {
  const s = setup(summary());
  expect(s.q(".ow-header__title").textContent).toBe("B동 203호");
  expect(s.q(".ow-header__sub").textContent).toBe("Claude Code CLI · WSL");
  expect(s.q(".ow-header__session").textContent).toBe("로그인 버그 고치기");
  expect(s.q(".ow-header__status").textContent).toBe("근무 중 · 12분째");
});

test("중소기업: 팀·직원 수, 세션 제목 없음", () => {
  const s = setup(summary({ building: null, label: "2층 Codex 사무실" }), "Small");
  expect(s.q(".ow-header__title").textContent).toBe("2층 Codex 사무실");
  expect(s.q(".ow-header__sub").textContent).toBe("팀 1 · 직원 3명");
  expect(s.q(".ow-header__session").hidden).toBe(true);
});

test("야근이면 야근 모드와 시각, 모두 퇴근이면 퇴근 문구", () => {
  const night = setup(summary(), "Large", {
    shared: { ...office("o", 1, []).shared, night: true },
  });
  expect(night.q(".ow-header__status").textContent).toBe("야근 모드 · 22:47");
  document.body.innerHTML = "";
  const off = setup(summary({ phase: "Off", teams: [{ ...summary().teams[0], phase: "Off" }] }));
  expect(off.q(".ow-header__status").textContent).toBe("모두 퇴근 · 22:47");
});

test("방 이름표 토글은 aria-pressed 를 바꾸고 렌더러에 알린다", () => {
  const s = setup(summary());
  const btn = s.q("[aria-pressed]");
  expect(btn.getAttribute("aria-pressed")).toBe("true");
  btn.click();
  expect(btn.getAttribute("aria-pressed")).toBe("false");
  expect(s.setRoomLabels).toHaveBeenLastCalledWith(false);
  btn.click();
  expect(s.setRoomLabels).toHaveBeenLastCalledWith(true);
});

test("panel 버튼은 1100 미만에서 보이고 드로어 상태와 맞는다", () => {
  const s = setup(summary());
  const btn = s.q(".ow-header__panel");
  expect(btn.hidden).toBe(true);
  s.vp.set(1000);
  expect(s.app.dataset.layout).toBe("drawer");
  expect(btn.hidden).toBe(false);
  expect(btn.getAttribute("aria-expanded")).toBe("false");
  btn.click();
  expect(btn.getAttribute("aria-expanded")).toBe("true");
  expect(s.panelHost.querySelector(".ow-panel")?.classList.contains("is-drawer")).toBe(true);
  btn.click();
  expect(btn.getAttribute("aria-expanded")).toBe("false");
  btn.click();
  s.vp.set(1200); // 넓어지면 드로어가 닫히고 버튼이 사라진다
  expect(btn.getAttribute("aria-expanded")).toBe("false");
  expect(btn.hidden).toBe(true);
});

test("720 미만에서는 panel 버튼이 사이드바 띠로 옮겨 가고, 구간이 바뀌어도 드로어와 맞는다", () => {
  const band = document.createElement("div");
  const s = setup(summary(), "Large", {}, () => band);
  const btn = s.q(".ow-header__panel");
  s.vp.set(800); // 52 띠: 머리말 안
  expect(btn.hidden).toBe(false);
  expect(btn.parentElement).toBe(s.header);
  btn.click();
  s.vp.set(600);
  expect(btn.parentElement).toBe(band);
  expect(btn.hidden).toBe(false);
  expect(btn.getAttribute("aria-expanded")).toBe("true");
  btn.click();
  expect(btn.getAttribute("aria-expanded")).toBe("false");
  s.vp.set(1000);
  expect(btn.parentElement).toBe(s.header);
});

test("상태 칩 점은 사이드바와 같은 판정: 장애·졸음·테두리", () => {
  const dot = (o: Partial<OfficeSummaryDto>) => setup(summary(o)).q(".ow-dot") as HTMLElement;
  expect(dot({ dot: "Incident" }).dataset.state).toBe("incident");
  expect(dot({ dot: "Dozing" }).dataset.state).toBe("sleepy");
  expect(dot({ border: "Failed" }).dataset.ring).toBe("fail");
  expect(dot({ delayed: true }).dataset.ring).toBe("lag");
  expect(dot({}).dataset.ring).toBeUndefined();
});

test("office 가 아직 없거나 시작 시각을 못 읽으면 상태 칩을 숨기고, 경과 시간은 시간·분", () => {
  const s = setup(summary());
  expect(s.q(".ow-header__status").parentElement!.hidden).toBe(false);
  s.store.selectOffice("o"); // office 를 비운다
  expect(s.q(".ow-header__status").parentElement!.hidden).toBe(true);
  document.body.innerHTML = "";
  const bad = setup(summary({ teams: [{ ...summary().teams[0], started: "x" }] }));
  expect(bad.q(".ow-header__status").parentElement!.hidden).toBe(true);
  document.body.innerHTML = "";
  const long = new Date(NOW.getTime() - (13 * 60 + 20) * 60_000).toISOString();
  const l = setup(summary({ teams: [{ ...summary().teams[0], started: long }] }));
  expect(l.q(".ow-header__status").textContent).toBe("근무 중 · 13시간 20분째");
});

test("창 너비 구간마다 배치: 띠는 레일에서, 머리말은 select 에서 숨는다", () => {
  const s = setup(summary());
  const at = (w: number) => {
    s.vp.set(w);
    return [s.app.dataset.layout, s.header.hidden, s.header.classList.contains("ow-header--strip")];
  };
  expect(at(1600)).toEqual(["wide", false, false]);
  expect(at(1000)).toEqual(["drawer", false, false]);
  expect(at(800)).toEqual(["rail", false, true]);
  expect(at(600)).toEqual(["select", true, false]);
  s.vp.set(800);
  expect(s.q(".ow-header__work").textContent).toBe("오늘의 업무 · 버그 수정");
});

test("결재 칩은 선택 사무실의 활성 팀 인원을 합하고, 퇴근·다른 사무실은 빼며 0이면 숨는다", () => {
  const base = summary().teams[0];
  const o = summary({
    teams: [
      { ...base, teamId: "a", askingCount: 2 },
      { ...base, teamId: "b", askingCount: 3 },
      { ...base, teamId: "off", phase: "Off", askingCount: 7 },
    ],
  });
  const s = setup(o, "Small");
  s.store.applyList({
    ...s.store.state.list!,
    listSeq: 2,
    offices: [o, summary({ officeId: "else", teams: [{ ...base, askingCount: 9 }] })],
  });
  expect(s.q(".ow-header__asking > span:last-child").textContent).toBe("결재 대기 5명");
  expect(s.q(".ow-header__asking").hidden).toBe(false);
  s.store.applyList({ ...s.store.state.list!, listSeq: 3, offices: [summary()] });
  expect(s.q(".ow-header__asking").hidden).toBe(true);
});

test("세그먼트의 이름·소리 상태와 미연결 야근 버튼은 aria-pressed로 표시한다", () => {
  const s = setup(summary());
  const buttons = s.header.querySelectorAll<HTMLButtonElement>(".ow-segment button");
  expect([...buttons].map((b) => b.getAttribute("aria-pressed"))).toEqual([
    "true",
    "false",
    "false",
  ]);
  expect(buttons[2].disabled).toBe(true);
  buttons[0].click();
  expect(s.setRoomLabels).toHaveBeenCalledWith(false);
  expect(buttons[0].getAttribute("aria-pressed")).toBe("false");
  s.handle!.setSettings(null, true);
  expect(buttons[1].getAttribute("aria-pressed")).toBe("true");
});

test("배율 메뉴는 허용 배율만 전달하고 선택·Escape에 닫히며 맞춤 한도만 바뀌어도 갱신된다", () => {
  const s = setup(summary());
  const trigger = s.q(".ow-header__scale");
  const choices = [...s.header.querySelectorAll<HTMLButtonElement>('[role="menuitemradio"]')];
  const menu = s.q('[role="menu"]');
  s.handle!.setScale(1, 2);
  expect(choices.map((b) => b.disabled)).toEqual([false, false, true]);
  trigger.click();
  expect(trigger.getAttribute("aria-expanded")).toBe("true");
  choices[2].click();
  expect(s.setScale).not.toHaveBeenCalled();
  expect(menu.hidden).toBe(false);
  choices[1].click();
  expect(s.setScale).toHaveBeenCalledExactlyOnceWith(2);
  expect(menu.hidden).toBe(true);
  expect(trigger.getAttribute("aria-expanded")).toBe("false");
  s.handle!.setScale(2, 2);
  expect(choices.map((b) => b.getAttribute("aria-checked"))).toEqual(["false", "true", "false"]);
  s.handle!.setScale(2, 3);
  expect(choices.map((b) => b.disabled)).toEqual([false, false, false]);
  trigger.click();
  choices[1].dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true }));
  expect(document.activeElement).toBe(choices[2]);
  choices[2].dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
  expect(menu.hidden).toBe(true);
  expect(trigger.getAttribute("aria-expanded")).toBe("false");
  expect(document.activeElement).toBe(trigger);
  trigger.click();
  s.vp.set(600);
  expect(menu.hidden).toBe(true);
  s.handle!.setScale(1, 1);
  expect(choices.map((b) => b.disabled)).toEqual([false, true, true]);
});

test("무대 도구는 가운데·최대화 동작을 호출하고 최대화 실패를 알린다", async () => {
  const s = setup(summary());
  s.stage.querySelector<HTMLButtonElement>('[aria-label="사무실 가운데로"]')!.click();
  expect(s.centerCanvas).toHaveBeenCalledOnce();
  const maximize = s.stage.querySelector<HTMLButtonElement>('[aria-label="창 최대화·복원"]')!;
  maximize.click();
  expect(s.toggleMaximize).toHaveBeenCalledOnce();
  expect(maximize.disabled).toBe(true);
  await Promise.resolve();
  expect(maximize.disabled).toBe(false);
  s.toggleMaximize.mockRejectedValueOnce(new Error("창 조작 실패"));
  maximize.click();
  await Promise.resolve();
  expect(s.app.querySelector('[role="alert"]')!.textContent).toContain("창 조작 실패");
});
