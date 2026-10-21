// @vitest-environment jsdom
import { afterEach, beforeEach, expect, test, vi } from "vite-plus/test";
import phrases from "../design/handoff/phrases.ko.json";
import ui from "../design/sprites/ui.json";
import { mountEmptyOffice } from "./empty-office.ts";
import type { Invoke } from "./scale.ts";
import { createStore } from "./store.ts";
import type { DiscoveryPathDto, OfficeDto, OfficeListDto, OfficeSummaryDto } from "./types/ipc.ts";

const cleanup: (() => void)[] = [];
const missing: DiscoveryPathDto = {
  host: "Windows",
  vendor: "Claude",
  path: "C:\\synthetic\\projects",
  status: "Missing",
};
const occupied: OfficeSummaryDto = {
  officeId: "o",
  building: "A",
  floor: null,
  room: 101,
  label: "A동 101호",
  phase: "Working",
  dot: "Working",
  asking: false,
  border: "Plain",
  delayed: false,
  teams: [],
};

beforeEach(() => {
  vi.useFakeTimers();
  vi.spyOn(document, "hidden", "get").mockReturnValue(false);
});
afterEach(() => {
  cleanup.splice(0).forEach((dispose) => dispose());
  vi.restoreAllMocks();
  vi.useRealTimers();
  document.body.replaceChildren();
});

function setup(read: () => Promise<DiscoveryPathDto[]> = async () => [missing]) {
  const store = createStore<OfficeListDto, OfficeDto>();
  const stage = document.body.appendChild(document.createElement("div"));
  const invoke = vi.fn((cmd: string) => {
    if (cmd !== "get_discovery") throw new Error(`예상 밖 명령 ${cmd}`);
    return read();
  });
  const onSettings = vi.fn();
  const handle = mountEmptyOffice(stage, store, {
    invoke: invoke as Invoke,
    onSettings,
    sheet: { frames: ui.frames, size: ui.meta.size },
  });
  cleanup.push(() => handle.dispose());
  const root = stage.querySelector<HTMLElement>(".ow-empty-office")!;
  let listSeq = 0;
  function list(offices: OfficeSummaryDto[] = []) {
    const msg: OfficeListDto = {
      listSeq: ++listSeq,
      rebuilt: false,
      scale: "Large",
      offices,
      selectedId: null,
      selectRequest: 0,
    };
    if (listSeq === 1) store.applyWorld({ ...msg, selected: null });
    else store.applyList(msg);
  }
  return { store, stage, root, list, invoke, onSettings, handle };
}

test("목록 전에는 숨기고, 후보 준비 중·모두 퇴근·폴더 없음·입주를 구분한다", async () => {
  let paths: DiscoveryPathDto[] = [];
  const s = setup(async () => paths);
  expect(s.root.hidden).toBe(true);
  expect(s.invoke).not.toHaveBeenCalled();
  s.list();
  await vi.advanceTimersByTimeAsync(0);
  expect(s.root.hidden).toBe(false);
  expect(s.root.textContent).toContain("탐색 정보를 준비하고 있습니다");
  expect(s.root.textContent).not.toContain(phrases.empty.noFoldersTitle);
  expect(s.invoke).toHaveBeenCalledExactlyOnceWith("get_discovery");

  paths = [missing, { ...missing, status: "Found" }];
  await vi.advanceTimersByTimeAsync(3000);
  expect(s.root.textContent).toContain(phrases.empty.allOffTitle);
  expect(s.root.textContent).toContain(phrases.empty.allOffBody);
  expect(s.root.querySelector<HTMLElement>(".ow-empty-office__building")?.style.width).toBe(
    "320px",
  );
  expect(s.root.querySelector("table")).toBeNull();

  paths = [missing];
  await vi.advanceTimersByTimeAsync(3000);
  expect(s.root.textContent).toContain(phrases.empty.noFoldersTitle);
  expect(s.root.textContent).toContain(missing.path);
  expect(s.root.querySelector("table")).not.toBeNull();

  s.list([occupied]);
  expect(s.root.hidden).toBe(true);
  await vi.advanceTimersByTimeAsync(6000);
  expect(s.invoke).toHaveBeenCalledTimes(3);
});

test("캐시 읽기는 동시에 하나이며 입주 뒤 늦은 응답은 빈 화면을 되살리지 않는다", async () => {
  let resolve!: (value: DiscoveryPathDto[]) => void;
  const s = setup(() => new Promise((done) => (resolve = done)));
  s.list();
  await vi.advanceTimersByTimeAsync(9000);
  expect(s.invoke).toHaveBeenCalledTimes(1);
  s.list([occupied]);
  resolve([missing]);
  await vi.advanceTimersByTimeAsync(0);
  expect(s.root.hidden).toBe(true);
  expect(s.root.querySelector("table")).toBeNull();
});

test("다시 빈 목록이 되면 이전 빈 목록의 응답을 버리고 현재 캐시를 읽는다", async () => {
  let resolve!: (value: DiscoveryPathDto[]) => void;
  const pending = new Promise<DiscoveryPathDto[]>((done) => (resolve = done));
  let reads = 0;
  const s = setup(() => (++reads === 1 ? pending : Promise.resolve([missing])));
  s.list();
  s.list([occupied]);
  s.list();
  resolve([{ ...missing, status: "Found" }]);
  await vi.advanceTimersByTimeAsync(0);
  expect(s.invoke).toHaveBeenCalledTimes(2);
  expect(s.root.hidden).toBe(false);
  expect(s.root.textContent).toContain(phrases.empty.noFoldersTitle);
  expect(s.root.textContent).not.toContain(phrases.empty.allOffTitle);
});

test("IPC 실패를 알리고 다음 캐시 조회가 성공하면 오류 안내를 걷는다", async () => {
  vi.spyOn(console, "error").mockImplementation(() => {});
  let fail = true;
  const s = setup(async () => {
    if (fail) throw new Error("합성 읽기 실패");
    return [missing];
  });
  s.list();
  await vi.advanceTimersByTimeAsync(0);
  expect(s.root.querySelector('[role="alert"]')?.textContent).toContain(
    "찾아본 경로를 불러오지 못했습니다",
  );
  expect(s.root.textContent).not.toContain(phrases.empty.noFoldersTitle);
  fail = false;
  await vi.advanceTimersByTimeAsync(3000);
  expect(s.root.querySelector('[role="alert"]')).toBeNull();
  expect(s.root.textContent).toContain(missing.path);
});

test("숨김 중에는 조회하지 않고 숨기기 전 응답도 복귀 화면에 쓰지 않는다", async () => {
  let resolve!: (value: DiscoveryPathDto[]) => void;
  const pending = new Promise<DiscoveryPathDto[]>((done) => (resolve = done));
  let reads = 0;
  const s = setup(() => (++reads === 1 ? pending : Promise.resolve([missing])));
  s.list();
  vi.spyOn(document, "hidden", "get").mockReturnValue(true);
  document.dispatchEvent(new Event("visibilitychange"));
  resolve([{ ...missing, status: "Found" }]);
  await vi.advanceTimersByTimeAsync(9000);
  expect(s.invoke).toHaveBeenCalledTimes(1);
  expect(s.root.textContent).not.toContain(phrases.empty.allOffTitle);
  vi.spyOn(document, "hidden", "get").mockReturnValue(false);
  document.dispatchEvent(new Event("visibilitychange"));
  await vi.advanceTimersByTimeAsync(0);
  expect(s.invoke).toHaveBeenCalledTimes(2);
  expect(s.root.textContent).toContain(phrases.empty.noFoldersTitle);
});

test("외부 경로·호스트·제조사는 글자로 표시하고 직접 지정은 연 버튼을 전달한다", async () => {
  const raw = '<img src=x onerror="alert(1)">';
  const s = setup(async () => [
    { ...missing, path: raw, host: raw, vendor: raw },
    { host: "WSL Preview", vendor: "Codex", path: null, status: "Offline" },
    { host: "WSL Unknown", vendor: "Claude", path: "\\\\wsl$\\Unknown\\home", status: "Unknown" },
  ]);
  s.list();
  await vi.advanceTimersByTimeAsync(0);
  expect(s.root.querySelector("img")).toBeNull();
  expect(s.root.querySelector("th")?.textContent).toBe(raw + raw);
  expect(s.root.querySelector(".ow-empty-office__path")?.textContent).toBe(raw);
  expect(s.root.textContent).toContain(phrases.empty.pathMissing);
  expect(s.root.textContent).toContain(phrases.empty.pathOff);
  expect(s.root.textContent).toContain(phrases.empty.pathOffBody);
  expect(s.root.textContent).toContain("경로 미확인");
  expect(s.root.textContent).toContain("확인 못함");
  const button = s.root.querySelector("button")!;
  button.focus();
  button.click();
  expect(s.onSettings).toHaveBeenCalledExactlyOnceWith(button);
  await vi.advanceTimersByTimeAsync(3000);
  expect(document.activeElement).toBe(button);
});

test("캔버스 실패로 해제한 뒤에는 늦은 응답과 주기 조회가 실패 화면을 덮지 않는다", async () => {
  let reject!: (reason: Error) => void;
  const s = setup(() => new Promise((_resolve, fail) => (reject = fail)));
  s.list();
  s.handle.dispose();
  s.stage.textContent = "캔버스 실패 안내";
  reject(new Error("늦은 합성 실패"));
  await vi.advanceTimersByTimeAsync(6000);
  expect(s.stage.textContent).toBe("캔버스 실패 안내");
  expect(s.invoke).toHaveBeenCalledTimes(1);
});
