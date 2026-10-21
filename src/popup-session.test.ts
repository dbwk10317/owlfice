// @vitest-environment jsdom
import { afterEach, expect, test, vi } from "vite-plus/test";
import phrases from "../design/handoff/phrases.ko.json";
import { openSessionDetail, type SessionPopupDeps } from "./popup-session.ts";
import { emp } from "./test-office.ts";
import type { EventDto, SessionDetailDto } from "./types/ipc.ts";

afterEach(() => {
  document.body.innerHTML = "";
});

const flush = () => new Promise((r) => setTimeout(r, 0));
const reject = (code: string) => Promise.reject(code);

function event(i: number, over: Partial<EventDto> = {}): EventDto {
  return {
    cursor: { generation: 1, byteOffset: i },
    agentId: "boss",
    ts: new Date(2026, 0, 1, 14, 31, i % 60).toISOString(),
    kind: "ToolCall",
    summary: `\`src/a${i}.ts\` 수정`,
    confidence: "Confirmed",
    tool: "Edit",
    callId: null,
    input: null,
    outcome: null,
    body: null,
    path: "/x/s.jsonl",
    lineNo: i + 1,
    lineIdx: 0,
    sidecar: null,
    test: null,
    ...over,
  };
}
const events = (n: number) => Array.from({ length: n }, (_, i) => event(i));

function dto(over: Partial<SessionDetailDto> = {}): SessionDetailDto {
  return {
    sessionId: "7c1e4a9b-2f6d-4c1a-9e0b-58d3a1f2c7e4",
    title: "사규",
    building: "B",
    hostLabel: "2층 WSL Rocky-9",
    workType: "BugFix",
    cwd: "\\\\wsl.localhost\\Rocky-9\\home\\dev\\work\\lumaweave",
    gitBranch: "fix/tail-offset",
    model: "claude-opus-5-5",
    started: new Date(2026, 0, 1, 14, 19, 0).toISOString(),
    lastActivity: new Date(2026, 0, 1, 14, 31, 0).toISOString(),
    turns: 7,
    tokens: { input: 182400, output: 9870 },
    headcount: { bujang: 1, gwajang: 1, daeri: 1, sawon: 2 },
    pr: { number: 142, url: "https://github.com/x/y/pull/142" },
    recent: events(20),
    recentError: null,
    ...over,
  };
}

function setup(handlers: { detail?: (limit: number) => unknown; open?: () => unknown } = {}) {
  const calls: [string, Record<string, unknown> | undefined][] = [];
  const invoke = vi.fn(async (cmd: string, args?: Record<string, unknown>) => {
    calls.push([cmd, args]);
    if (cmd === "get_session_detail")
      return (handlers.detail ?? (() => dto()))(args!.limit as number);
    if (cmd === "open_path") return handlers.open?.();
    throw new Error(`예상 밖 커맨드 ${cmd}`);
  });
  const copy = vi.fn(async (_text: string) => {});
  Object.defineProperty(navigator, "clipboard", {
    value: { writeText: copy },
    configurable: true,
  });
  const deps: SessionPopupDeps = {
    invoke: invoke as unknown as SessionPopupDeps["invoke"],
    live: (teamId, agentId) =>
      teamId === "t1" && agentId === "boss"
        ? emp("boss", { name: "김하늘", rank: "Bujang" })
        : undefined,
    now: () => new Date(2026, 0, 1, 14, 31, 0).getTime(),
  };
  const opener = document.createElement("button");
  document.body.append(opener);
  opener.focus();
  openSessionDetail(deps, { officeId: "o1", teamId: "t1", name: "B동 203호" }, opener);
  const q = <T extends Element = HTMLElement>(sel: string) => document.querySelector<T>(sel);
  const qa = (sel: string) => [...document.querySelectorAll<HTMLElement>(sel)];
  return { calls, invoke, copy, q, qa, opener };
}

test("머리·정보 표·최근 사건 20개를 get_session_detail(limit 20) 로 불러 그린다", async () => {
  const s = setup();
  expect(s.calls[0]).toEqual(["get_session_detail", { officeId: "o1", teamId: "t1", limit: 20 }]);
  await flush();
  expect(s.q("#sd-title")!.textContent).toBe("B동 203호");
  expect(s.q(".ow-modal .ow-chip")!.textContent).toBe("버그 수정");
  expect(s.q(".ow-pop__sub")!.textContent).toBe("B동 · Claude Code CLI · 2층 WSL Rocky-9");

  expect(s.qa(".ow-kv dt").map((e) => e.textContent)).toEqual([
    "세션 ID",
    "브랜치",
    "작업 폴더",
    "모델",
    "시작 · 경과",
    "턴 수",
    "토큰 사용량",
    "직원 수",
    "PR",
  ]);
  const dd = (label: string) =>
    s
      .qa(".ow-kv__item")
      .find((i) => i.querySelector("dt")!.textContent === label)!
      .querySelector("dd")!.textContent;
  expect(dd("세션 ID")).toBe("7c1e4a9b-2f6d-4c1a-9e0b-58d3a1f2c7e4");
  expect(dd("작업 폴더")).toBe("\\\\wsl.localhost\\Rocky-9\\home\\dev\\work\\lumaweave");
  expect(dd("시작 · 경과")).toContain("14:19 · 12분째");
  expect(dd("토큰 사용량")).toBe("입력 182,400 · 출력 9,870");
  expect(dd("직원 수")).toBe("5명 (부장 1 · 과장 1 · 대리 1 · 사원 2)");
  expect(dd("PR")).toContain("#142");

  expect(s.qa(".ow-tl")).toHaveLength(20);
  expect(s.qa(".ow-tl")[0].querySelector(".ow-tl__who")!.textContent).toBe("김하늘 부장");
  expect(s.qa(".ow-tl")[0].querySelector(".ow-tl__what .ow-code")!.textContent).toBe("src/a0.ts");
  expect(s.q(".ow-pop__more")!.textContent).toBe("더 보기최대 100개");
});

test("토큰·PR 이 없으면 그 칸을 빼고, 사장님·모르는 직원·추정을 구분한다", async () => {
  const s = setup({
    detail: () =>
      dto({
        tokens: null,
        pr: null,
        gitBranch: null,
        recent: [
          event(1, { kind: "UserMessage", agentId: "x" }),
          event(2, { agentId: "ghost", confidence: "Inferred" }),
        ],
      }),
  });
  await flush();
  const labels = s.qa(".ow-kv dt").map((e) => e.textContent);
  expect(labels).not.toContain("토큰 사용량");
  expect(labels).not.toContain("PR");
  const rows = s.qa(".ow-tl");
  expect(rows.map((r) => r.querySelector(".ow-tl__who")!.textContent)).toEqual(["사장님", "ghost"]);
  expect(rows[0].querySelector(".ow-est-tag")).toBeNull();
  expect(rows[1].querySelector(".ow-est-tag")!.textContent).toBe("추정");
  // 20개를 못 채웠으면 더 있을 수 없다
  expect(s.q(".ow-pop__more")).toBeNull();
});

test("더 보기는 limit 100 으로 한 번 더 불러 목록을 바꾸고 버튼을 없앤다", async () => {
  const s = setup({
    detail: (limit) => dto({ recent: events(limit === 100 ? 100 : 20) }),
  });
  await flush();
  const kv = s.q(".ow-kv");
  s.q<HTMLButtonElement>(".ow-pop__more")!.click();
  expect(s.calls[1]).toEqual(["get_session_detail", { officeId: "o1", teamId: "t1", limit: 100 }]);
  await flush();
  expect(s.qa(".ow-tl")).toHaveLength(100);
  expect(s.q(".ow-pop__more")).toBeNull();
  expect(s.q(".ow-kv")).toBe(kv); // 정보 표는 다시 만들지 않는다
});

test("원본 줄을 못 읽으면 최근 사건 자리에 그 오류 문구, 나머지는 그대로", async () => {
  const s = setup({ detail: () => dto({ recent: [], recentError: "host_offline" }) });
  await flush();
  expect(s.qa(".ow-tl")).toHaveLength(0);
  expect(s.q(".ow-pop__timeline .ow-pop__error")!.textContent).toBe(
    phrases.detailError.host_offline,
  );
  expect(s.q(".ow-kv")).not.toBeNull();
  expect(s.q(".ow-pop__more")).toBeNull();
});

test("불러오기가 거절되면 코드별 문구를 보인다", async () => {
  const s = setup({ detail: () => reject("not_found") });
  await flush();
  expect(s.q(".ow-pop__body .ow-pop__error")!.textContent).toBe(phrases.detailError.not_found);
  expect(s.q(".ow-kv")).toBeNull();
});

test("더 보기가 원본을 못 읽으면(Ok + recentError) 받아 둔 목록은 두고 오류만 보인 뒤 다시 누를 수 있다", async () => {
  let n = 0;
  const s = setup({
    detail: (limit) =>
      limit === 100 && n++ === 0
        ? dto({ recent: [], recentError: "timeout" })
        : dto({ recent: events(limit === 100 ? 100 : 20) }),
  });
  await flush();
  s.q<HTMLButtonElement>(".ow-pop__more")!.click();
  await flush();
  expect(s.qa(".ow-tl")).toHaveLength(20);
  expect(s.q(".ow-pop__timeline .ow-pop__error")!.textContent).toBe(phrases.detailError.timeout);
  expect(s.q<HTMLButtonElement>(".ow-pop__more")!.disabled).toBe(false);
  s.q<HTMLButtonElement>(".ow-pop__more")!.click();
  await flush();
  expect(s.qa(".ow-tl")).toHaveLength(100);
  expect(s.q(".ow-pop__timeline .ow-pop__error")).toBeNull();
});

test("더 보기 호출 자체가 거절돼도 목록은 두고 코드별 문구를 보인다", async () => {
  const s = setup({ detail: (limit) => (limit === 100 ? reject("host_offline") : dto()) });
  await flush();
  s.q<HTMLButtonElement>(".ow-pop__more")!.click();
  await flush();
  expect(s.qa(".ow-tl")).toHaveLength(20);
  expect(s.q(".ow-pop__timeline .ow-pop__error")!.textContent).toBe(
    phrases.detailError.host_offline,
  );
});

test("세션 ID 복사", async () => {
  const s = setup();
  await flush();
  s.q<HTMLButtonElement>('[aria-label="세션 ID 복사"]')!.click();
  await flush();
  expect(s.copy).toHaveBeenCalledWith("7c1e4a9b-2f6d-4c1a-9e0b-58d3a1f2c7e4");
});

test("바닥 버튼은 open_path 로 원본 폴더·작업 폴더를 열고 실패하면 문구를 보인다", async () => {
  let fail = false;
  const s = setup({ open: () => (fail ? reject("open_failed") : undefined) });
  await flush();
  const [raw, work] = s.qa(".ow-modal__foot .ow-btn");
  expect([raw.textContent, work.textContent]).toEqual(["원본 파일 폴더 열기", "작업 폴더 열기"]);
  raw.click();
  work.click();
  await flush();
  expect(s.calls.filter(([c]) => c === "open_path").map(([, a]) => a)).toEqual([
    { target: { kind: "SourceFolder", officeId: "o1", teamId: "t1" } },
    { target: { kind: "WorkFolder", officeId: "o1", teamId: "t1" } },
  ]);
  const note = s.q(".ow-pop__note")!;
  expect(note.hidden).toBe(true);
  fail = true;
  raw.click();
  await flush();
  expect(note.hidden).toBe(false);
  expect(note.textContent).toBe(phrases.detailError.open_failed);
});

test("PR 줄을 누르면 주소 없이 그 팀의 Pr 대상으로 open_path 를 부르고 실패하면 PR 문구를 보인다", async () => {
  let fail = false;
  const s = setup({ open: () => (fail ? reject("open_failed") : undefined) });
  await flush();
  const pr = s.q<HTMLButtonElement>(".ow-kv__pr")!;
  expect(pr.tagName).toBe("BUTTON");
  expect(pr.textContent).toBe("#142 https://github.com/x/y/pull/142");
  pr.click();
  await flush();
  expect(s.calls.filter(([c]) => c === "open_path").map(([, a]) => a)).toEqual([
    { target: { kind: "Pr", officeId: "o1", teamId: "t1" } },
  ]);
  const note = s.q(".ow-pop__note")!;
  expect(note.hidden).toBe(true);
  fail = true;
  pr.click();
  await flush();
  expect(note.hidden).toBe(false);
  expect(note.textContent).toBe("PR 주소를 열지 못했어요.");
});

test("PR 줄 실패가 open_failed 가 아니면 코드별 문구를 보인다", async () => {
  const s = setup({ open: () => reject("not_found") });
  await flush();
  s.q<HTMLButtonElement>(".ow-kv__pr")!.click();
  await flush();
  expect(s.q(".ow-pop__note")!.textContent).toBe(phrases.detailError.not_found);
});

test("Esc 로 닫으면 연 버튼으로 초점이 돌아가고, 닫은 뒤 늦게 온 응답은 그리지 않는다", async () => {
  let resolve!: (d: SessionDetailDto) => void;
  const late = new Promise<SessionDetailDto>((r) => (resolve = r));
  const s = setup({ detail: () => late });
  document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
  expect(s.q(".ow-backdrop")).toBeNull();
  expect(document.activeElement).toBe(s.opener);
  resolve(dto());
  await flush();
  expect(s.q(".ow-kv")).toBeNull();
});
