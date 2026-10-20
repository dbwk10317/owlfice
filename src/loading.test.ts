// @vitest-environment jsdom
import { afterEach, beforeEach, expect, test, vi } from "vite-plus/test";
import phrases from "../design/handoff/phrases.ko.json";
import { showLoading, type LoadingSheet } from "./loading.ts";
import type { StartupDetail, StartupProgressDto, StartupSummary } from "./types/ipc.ts";

const L = phrases.loading;
type Handler = (e: { payload: StartupProgressDto }) => void;

beforeEach(() => vi.useFakeTimers());
afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

function ev(
  step: number,
  state: StartupProgressDto["state"],
  summary: StartupSummary = { kind: "None" },
  detail: StartupDetail | null = null,
): StartupProgressDto {
  return { step, state, summary, detail };
}

/** 지금까지 기록(history)을 가진 백엔드와 이벤트 통로 */
function mount(history: StartupProgressDto[] = [], sheet?: LoadingSheet) {
  let handler: Handler = () => {};
  const unlisten = vi.fn();
  const io = {
    invoke: vi.fn().mockResolvedValue(history),
    listen: vi.fn(async (_: string, h: Handler) => {
      handler = h;
      return unlisten;
    }),
    random: () => 0,
  };
  const root = document.createElement("div");
  const done = showLoading(root, io as never, sheet);
  const state = { resolved: false };
  void done.then(() => (state.resolved = true));
  return {
    root,
    io,
    unlisten,
    state,
    send: (...list: StartupProgressDto[]) => list.forEach((p) => handler({ payload: p })),
    rows: () => [...root.querySelectorAll<HTMLElement>(".ld__step")],
    text: () => [...root.querySelectorAll<HTMLElement>(".ld__step")].map((r) => r.textContent),
    flush: () => vi.advanceTimersByTimeAsync(0),
  };
}

const allDone = () => [1, 2, 3, 4, 5, 6, 7].map((s) => ev(s, "Done"));

test("처음엔 7단계가 순서대로 빈 원과 기본 문구로 보이고, 끝난 단계는 한 줄로 바뀐다", async () => {
  const m = mount();
  await m.flush();
  expect(m.rows()).toHaveLength(7);
  expect(m.text()).toEqual(L.steps.map((s) => s.pool[0]));
  expect(m.io.listen).toHaveBeenCalledWith("startup-progress", expect.any(Function));

  m.send(
    ev(1, "Done"),
    ev(2, "Done", { kind: "Candidates", n: 2, hosts: ["Windows", "WSL Rocky-9"] }),
    ev(3, "Done", { kind: "Rented", buildings: ["Windows Claude", "WSL Rocky-9 Codex"] }),
    ev(4, "Done", { kind: "Teams", n: 4 }),
    ev(5, "Running"),
  );
  const t = m.text();
  expect(t[0]).toBe(L.steps[0].done);
  expect(t[1]).toBe("후보지 2곳 확인 (Windows, WSL Rocky-9)");
  expect(t[2]).toBe("Windows Claude · WSL Rocky-9 Codex 임대 계약 완료");
  expect(t[3]).toBe("불 켜진 팀 4곳");
  expect(t[4]).toBe(`${L.steps[4].pool[0]}.`);
  expect(t[5]).toBe(L.steps[5].pool[0]);
  expect(m.rows().map((r) => r.className)).toEqual([
    "ld__step ld__step--done",
    "ld__step ld__step--done",
    "ld__step ld__step--done",
    "ld__step ld__step--done",
    "ld__step ld__step--running",
    "ld__step ld__step--todo",
    "ld__step ld__step--todo",
  ]);
  m.send(ev(5, "Done", { kind: "Hired", n: 12, bujang: 4, gwajang: 2, daeri: 3, sawon: 3 }));
  expect(m.text()[4]).toBe("12명 입사 (부장 4, 과장 2, 대리 3, 사원 3)");
});

test("진행 중 줄은 점이 시간에 맞춰 늘고, 2초 넘으면 원인, 5초 넘으면 농담이 붙는다", async () => {
  const m = mount();
  await m.flush();
  m.send(ev(5, "Running", { kind: "None" }, { kind: "Files", done: 187, total: 312 }));
  const row = () => m.rows()[4];
  const phrase = L.steps[4].pool[0];
  expect(row().textContent).toBe(`${phrase}.`);
  await vi.advanceTimersByTimeAsync(600);
  expect(row().textContent).toBe(`${phrase}..`);
  await vi.advanceTimersByTimeAsync(600);
  expect(row().textContent).toBe(`${phrase}...`);
  await vi.advanceTimersByTimeAsync(700);
  expect(row().textContent).toBe(`${phrase}...`);
  // 1999ms: 아직 원인 없음
  expect(row().querySelector(".ld__cause")).toBeNull();
  await vi.advanceTimersByTimeAsync(100);
  expect(row().querySelector(".ld__cause")?.textContent).toBe("세션 파일 312개 확인 중 (187/312)");
  expect(row().querySelector(".ld__slow")).toBeNull();
  await vi.advanceTimersByTimeAsync(2900);
  expect(row().querySelector(".ld__slow")).toBeNull();
  await vi.advanceTimersByTimeAsync(100);
  expect(row().querySelector(".ld__slow")?.textContent).toBe(L.slow);
});

test("원인이 없으면 2초가 넘어도 원인 줄은 없고 5초 농담만 붙는다", async () => {
  const m = mount();
  await m.flush();
  m.send(ev(2, "Running"));
  await vi.advanceTimersByTimeAsync(2500);
  expect(m.rows()[1].querySelector(".ld__cause")).toBeNull();
  await vi.advanceTimersByTimeAsync(2500);
  expect(m.rows()[1].querySelector(".ld__slow")).not.toBeNull();
});

test("전부 빨리 끝나도 1.2초는 보여 주고, 간판 0.3초 뒤 0.3초 페이드로 풀린다", async () => {
  const m = mount(allDone());
  await m.flush();
  expect(m.text()).toHaveLength(7);
  await vi.advanceTimersByTimeAsync(1100);
  expect(m.root.querySelector(".ld--fade")).toBeNull();
  await vi.advanceTimersByTimeAsync(100);
  // 1.2초: 간판 단계. 아직 페이드 전
  expect(m.root.querySelector(".ld--fade")).toBeNull();
  await vi.advanceTimersByTimeAsync(299);
  expect(m.root.querySelector(".ld--fade")).toBeNull();
  await vi.advanceTimersByTimeAsync(1);
  expect(m.root.querySelector(".ld--fade")).not.toBeNull();
  expect(m.state.resolved).toBe(false);
  await vi.advanceTimersByTimeAsync(300);
  expect(m.state.resolved).toBe(true);
  expect(m.unlisten).toHaveBeenCalledTimes(1);
});

test("7단계가 안 끝났으면 1.2초가 지나도 기다린다", async () => {
  const m = mount(allDone().slice(0, 6));
  await m.flush();
  await vi.advanceTimersByTimeAsync(5000);
  expect(m.state.resolved).toBe(false);
  m.send(ev(7, "Done"));
  await vi.advanceTimersByTimeAsync(600);
  expect(m.state.resolved).toBe(true);
});

test("아무 키나 클릭으로 건너뛰지만 Tab·Shift 같은 보조 키는 안 된다", async () => {
  const key = (k: string) => document.dispatchEvent(new KeyboardEvent("keydown", { key: k }));
  const m = mount();
  await m.flush();
  for (const k of ["Tab", "Shift", "Control", "Alt", "Meta", "CapsLock"]) key(k);
  await vi.advanceTimersByTimeAsync(1000);
  expect(m.root.querySelector(".ld--fade")).toBeNull();
  key("a");
  expect(m.root.querySelector(".ld--fade")).not.toBeNull();
  await vi.advanceTimersByTimeAsync(300);
  expect(m.state.resolved).toBe(true);

  const c = mount();
  await c.flush();
  document.dispatchEvent(new MouseEvent("click"));
  await vi.advanceTimersByTimeAsync(300);
  expect(c.state.resolved).toBe(true);
});

test("꺼진 WSL 은 경고가 아닌 소등 줄, 접근 실패와 폴더 없음은 노란 경고", async () => {
  const m = mount();
  await m.flush();
  m.send(
    ev(3, "Done", { kind: "WslOff", floor: 2, distro: "Rocky-9" }),
    ev(3, "Warn", { kind: "WslFail", floor: 3, distro: "Ubuntu" }),
    ev(3, "Error", { kind: "NoFolders" }),
    ev(1, "Warn", { kind: "SettingsBroken" }),
  );
  const rows = m.rows();
  // 단계 1 + 알림, 2, 단계 3 + 알림 둘, 4 ... 7
  expect(rows).toHaveLength(7 + 3);
  const byText = (t: string) => rows.find((r) => r.textContent === t)!;
  const off = byText("2층 소등 (Rocky-9 꺼짐)");
  expect(off.classList.contains("ld__step--off")).toBe(true);
  expect(off.classList.contains("ld__step--warn")).toBe(false);
  expect(byText("3층 임대 실패 (WSL Ubuntu 응답 없음)").classList.contains("ld__step--warn")).toBe(
    true,
  );
  expect(byText(phrases.empty.settingsBroken).classList.contains("ld__step--warn")).toBe(true);
  expect(byText(phrases.empty.noFoldersTitle).classList.contains("ld__step--warn")).toBe(true);
  // 알림 줄은 자기 단계 바로 밑에 붙는다
  expect(rows[1].textContent).toBe(phrases.empty.settingsBroken);
  expect(rows[4].textContent).toBe("2층 소등 (Rocky-9 꺼짐)");
});

test("구독과 기록이 겹쳐도, 늦게 온 옛 진행이 끝난 단계를 되돌리지 않는다", async () => {
  const m = mount([ev(1, "Done"), ev(2, "Done", { kind: "Candidates", n: 1, hosts: ["Windows"] })]);
  await m.flush();
  expect(m.text()[1]).toBe("후보지 1곳 확인 (Windows)");
  m.send(ev(2, "Running"), ev(1, "Running"));
  expect(m.text()[1]).toBe("후보지 1곳 확인 (Windows)");
  expect(m.rows()[0].classList.contains("ld__step--done")).toBe(true);
  // 같은 알림 줄이 기록과 구독으로 두 번 와도 한 줄이다
  m.send(ev(3, "Done", { kind: "WslOff", floor: 2, distro: "R" }));
  m.send(ev(3, "Done", { kind: "WslOff", floor: 2, distro: "R" }));
  expect(m.rows()).toHaveLength(8);
});

test("다시 불러온 기록은 이미 이벤트로 받은 줄의 원인을 옛 값으로 덮지 않는다", async () => {
  let resolve!: (h: StartupProgressDto[]) => void;
  const late = new Promise<StartupProgressDto[]>((r) => (resolve = r));
  const m = mount(late as never);
  await m.flush();
  const files = (done: number) =>
    ev(5, "Running", { kind: "None" }, { kind: "Files", done, total: 312 });
  m.send(files(200));
  resolve([files(100)]);
  await m.flush();
  await vi.advanceTimersByTimeAsync(2000);
  expect(m.rows()[4].querySelector(".ld__cause")?.textContent).toBe(
    "세션 파일 312개 확인 중 (200/312)",
  );
});

test("키를 누른 채 생기는 자동 반복 keydown 은 건너뛰기가 아니다", async () => {
  const m = mount();
  await m.flush();
  document.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", repeat: true }));
  expect(m.root.querySelector(".ld--fade")).toBeNull();
  document.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter" }));
  expect(m.root.querySelector(".ld--fade")).not.toBeNull();
});

const sheet: LoadingSheet = {
  size: { w: 100, h: 100 },
  frames: {
    "building/base": { frame: { x: 0, y: 0, w: 10, h: 10 } },
    "building/window_lit": { frame: { x: 0, y: 20, w: 2, h: 2 } },
    "building/window_eyes": { frame: { x: 4, y: 20, w: 2, h: 2 } },
    "building/sign_on": { frame: { x: 0, y: 30, w: 4, h: 2 } },
  },
  building: {
    windows: [
      [0, 0],
      [2, 0],
      [4, 0],
      [6, 0],
      [8, 0],
      [0, 4],
      [2, 4],
    ],
    lightOrder: [0, 1, 2, 3, 4, 5, 6],
    signOffset: [1, 1],
  },
};

test("단계가 끝난 만큼 lightOrder 순서로 창에 불이 켜지고, 꺼진 창 몇 곳에 눈이 뜨이고, 끝나면 간판이 켜진다", async () => {
  const m = mount([], sheet);
  await m.flush();
  const on = (name: string) =>
    [...m.root.querySelectorAll<HTMLElement>(".ld__win.is-on")].filter((e) =>
      e.style.backgroundPosition.startsWith(name),
    );
  // 창 7개, 단계 7개라 단계 하나에 창 하나. 눈은 켜진 창 다음 셋
  m.send(ev(1, "Done"), ev(2, "Done"), ev(3, "Done"));
  const lit = [...m.root.querySelectorAll<HTMLElement>(".ld__win.is-on")];
  // 불 3 + 눈 3
  expect(lit).toHaveLength(6);
  expect(on("0px -40px")).toHaveLength(3);
  expect(on("-8px -40px")).toHaveLength(3);
  m.send(...[4, 5, 6, 7].map((s) => ev(s, "Done")));
  expect(on("0px -40px")).toHaveLength(7);
  expect(on("-8px -40px")).toHaveLength(0);
  await vi.advanceTimersByTimeAsync(1200);
  expect(on("0px -60px")).toHaveLength(1);
});

test("움직임 줄이기: 페이드 없이 간판 뒤 바로 넘어간다", async () => {
  vi.stubGlobal("matchMedia", (q: string) => ({ matches: q.includes("reduce") }));
  const m = mount(allDone());
  await m.flush();
  await vi.advanceTimersByTimeAsync(1500);
  expect(m.state.resolved).toBe(true);
  expect(m.root.querySelector(".ld--fade")).toBeNull();
});
