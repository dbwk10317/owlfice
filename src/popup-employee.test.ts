// @vitest-environment jsdom
import { afterEach, expect, test, vi } from "vite-plus/test";
import userEvent from "@testing-library/user-event";
import phrases from "../design/handoff/phrases.ko.json";
import { openEmployeeDetail, type EmployeePopupDeps } from "./popup-employee.ts";
import { emp } from "./test-office.ts";
import type { EmployeeDetailDto, EmployeeDto, EventDto } from "./types/ipc.ts";
import { setAvatar } from "./avatar.ts";

vi.mock("./avatar.ts", () => ({ setAvatar: vi.fn() }));

afterEach(() => {
  document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
  document.body.innerHTML = "";
});

const flush = () => new Promise((r) => setTimeout(r, 0));
const reject = (code: string) => Promise.reject(code);

function event(i: number, over: Partial<EventDto> = {}): EventDto {
  return {
    cursor: { generation: 1, byteOffset: 1000 - i },
    agentId: "a",
    ts: new Date(2026, 0, 1, 14, 31, 59 - (i % 60)).toISOString(),
    kind: "ToolCall",
    summary: `\`src/a${i}.ts\` 수정`,
    confidence: "Confirmed",
    tool: "Edit",
    callId: null,
    input: null,
    outcome: null,
    body: null,
    path: "/x/subagents/agent-a.jsonl",
    lineNo: 200 + i,
    lineIdx: 1,
    sidecar: null,
    test: null,
    ...over,
  };
}
const page = (from: number, n: number) => Array.from({ length: n }, (_, i) => event(from + i));

const detail: EmployeeDetailDto = {
  agentId: "a",
  name: "박서준",
  rank: "Gwajang",
  role: "Backend",
  model: "claude-opus-5-5",
  hiredAt: new Date(2026, 0, 1, 14, 28, 0).toISOString(),
  retiredAt: null,
  bossName: "김하늘 부장",
  instruction: "tail.rs 의 오프셋 되감기 수정이 맞는지 확인해줘.",
  filesRead: ["src/tail.rs", "Cargo.toml"],
  filesEdited: [{ item: "src/tail_test.rs", times: 2 }],
  commands: [
    { item: "cargo test", times: 3 },
    { item: "cargo fmt", times: 1 },
    { item: "git diff", times: 1 },
  ],
};

type Handlers = {
  detail?: () => unknown;
  timeline?: (args: Record<string, unknown>) => unknown;
  open?: () => unknown;
  live?: EmployeeDto | null;
  off?: boolean;
};
function setup(handlers: Handlers = {}, teamId = "t1") {
  const calls: [string, Record<string, unknown> | undefined][] = [];
  const invoke = vi.fn(async (cmd: string, args?: Record<string, unknown>) => {
    calls.push([cmd, args]);
    if (cmd === "get_employee_detail") return (handlers.detail ?? (() => detail))();
    if (cmd === "get_employee_timeline") return (handlers.timeline ?? (() => page(0, 3)))(args!);
    if (cmd === "open_path") return handlers.open?.();
    throw new Error(`예상 밖 커맨드 ${cmd}`);
  });
  const deps: EmployeePopupDeps = {
    invoke: invoke as unknown as EmployeePopupDeps["invoke"],
    live: (t, id) =>
      t === "t1" && id === "a" && handlers.live !== null
        ? (handlers.live ?? emp("a", { contextUsed: 150_000, contextWindow: 200_000 }))
        : undefined,
    isTeamOff: () => handlers.off ?? false,
    now: () => new Date(2026, 0, 1, 14, 31, 0).getTime(),
  };
  const opener = document.createElement("button");
  document.body.append(opener);
  opener.focus();
  openEmployeeDetail(
    deps,
    { officeId: "o1", teamId, agentId: "a", officeName: "B동 203호", building: "D" },
    opener,
  );
  const q = <T extends Element = HTMLElement>(sel: string) => document.querySelector<T>(sel);
  const qa = (sel: string) => [...document.querySelectorAll<HTMLElement>(sel)];
  return { calls, q, qa, opener };
}

test("실시간 직원이 없어도 상세 아바타는 명시한 팀 제조사와 응답 직원을 쓴다", async () => {
  const s = setup({}, "not-live");
  expect(s.q(".ow-pop__body")?.hasAttribute("aria-live")).toBe(false);
  await flush();
  expect(setAvatar).toHaveBeenLastCalledWith(s.q(".ow-avatar"), detail, "D");
});

test("(teamId, agentId) 로 요약과 타임라인 첫 50개를 불러 머리·지시·요약을 그린다", async () => {
  const s = setup();
  expect(s.calls).toEqual([
    ["get_employee_detail", { officeId: "o1", teamId: "t1", agentId: "a" }],
    [
      "get_employee_timeline",
      { officeId: "o1", teamId: "t1", agentId: "a", before: null, limit: 50 },
    ],
  ]);
  await flush();
  expect(s.q("#ed-title")!.textContent).toBe("박서준");
  expect(s.q(".ow-pop__rank")!.textContent).toBe("과장");
  expect(s.q(".ow-avatar--xl")).not.toBeNull();
  expect(s.q(".ow-emp__work.is-live")!.textContent).toBe("근무 중 · 3분째");
  const chips = s.qa(".ow-pop__chips .ow-chip").map((c) => c.textContent);
  expect(chips[0]).toBe("담당 백엔드");
  expect(chips[1]).toBe("claude-opus-5-5");
  expect(chips[chips.length - 1]).toContain("75%"); // 컨텍스트
  expect(s.q(".ow-pop__meta")!.textContent).toBe(
    "입사 14:28 · 근속 3분 · 상사 김하늘 부장 · B동 203호",
  );
  expect(s.q(".ow-pop__instruction")!.textContent).toBe(detail.instruction);
  expect(s.qa(".ow-emp__aside .ow-pop__h2").map((h) => h.textContent)).toEqual([
    "실적 요약",
    "읽은 파일",
    "수정한 파일",
    "실행한 명령",
  ]);
  expect(s.qa(".ow-emp__li").map((e) => e.textContent)).toEqual([
    "src/tail.rs",
    "Cargo.toml",
    "src/tail_test.rs2회",
    "cargo test3회",
    "cargo fmt1회",
    "git diff1회",
  ]);
  // 읽은 파일은 횟수 없이, 수정한 파일·명령은 항목마다 "n회"
  expect(s.qa(".ow-emp__li .ow-emp__times").map((e) => e.textContent)).toEqual([
    "2회",
    "3회",
    "1회",
    "1회",
  ]);
  expect(s.qa(".ow-stat__head b").map((e) => e.textContent)).toEqual(["2", "2", "5"]);
  expect(s.qa(".ow-tl__row")).toHaveLength(3);
  expect(s.q(".ow-pop__tlhead .ow-pop__n")!.textContent).toBe("3개");
});

test("부장은 상사 토막이 빠지고, 요약이 거절되면 코드별 문구만 보인다", async () => {
  let s = setup({ detail: () => ({ ...detail, bossName: null, instruction: null }) });
  await flush();
  expect(s.q(".ow-pop__meta")!.textContent).toBe("입사 14:28 · 근속 3분 · B동 203호");
  expect(s.q(".ow-pop__instruction")).toBeNull();
  document.body.innerHTML = "";

  s = setup({ detail: () => reject("not_found") });
  await flush();
  expect(s.q(".ow-pop__body .ow-pop__error")!.textContent).toBe(phrases.detailError.not_found);
  expect(s.q(".ow-tl__row")).toBeNull();
});

test("접힌 행은 시각·도구 칩·요약·결과색, 펼치면 입력·결과 원문과 가운데 생략", async () => {
  const long = "x".repeat(4096);
  const s = setup({
    timeline: () => [
      event(0, {
        summary: "`cargo test` 실행",
        tool: "Bash",
        input: { head: '{ "command": "cargo test" }', tail: "", bytes: 27 },
      }),
      event(1, {
        kind: "ToolResult",
        tool: null,
        summary: "합격 11 / 반려 1",
        outcome: "Err",
        body: { head: "running 12 tests", tail: "test result: FAILED", bytes: 3200 },
        sidecar: "/x/tool-results/big.txt",
      }),
      event(2, { kind: "ToolResult", tool: null, outcome: "Ok", confidence: "Inferred" }),
      event(3, { outcome: "Started", ts: new Date(2026, 0, 1, 14, 29, 59).toISOString() }),
      event(4, { kind: "ToolResult", tool: null, outcome: "Unknown" }),
      event(5, { kind: "UserMessage", tool: null, body: { head: long, tail: "", bytes: 4096 } }),
      event(6, {
        kind: "ToolResult",
        tool: null,
        outcome: "Err",
        test: { passed: 11, failed: 1 },
      }),
      event(7, { kind: "ToolResult", tool: null, outcome: "Ok", test: { passed: 12, failed: 0 } }),
      event(8, { kind: "SubagentSpawn", tool: null }),
      event(9, { kind: "PrLinked", tool: null }),
      event(10, { kind: "ToolResult", tool: null, summary: "반려", outcome: "Rejected" }),
    ],
  });
  await flush();
  const rows = s.qa(".ow-tl__row");
  expect(rows).toHaveLength(11);
  const head = (i: number) => rows[i].querySelector<HTMLElement>(".ow-tl__head")!;
  expect(head(0).querySelector(".ow-tool")!.textContent).toBe("Bash");
  expect(head(0).querySelector(".ow-tl__summary .ow-code")!.textContent).toBe("cargo test");
  expect(head(0).querySelector(".ow-tl__time")!.textContent).toBe("14:31:59");
  expect(head(0).getAttribute("aria-expanded")).toBe("false");
  expect(rows[0].querySelector<HTMLElement>(".ow-tl__raw")!.hidden).toBe(true);

  const res = (i: number) => head(i).querySelector<HTMLElement>(".ow-tl__result")!;
  expect(res(1).classList.contains("ow-tl__bad")).toBe(true);
  expect(res(1).textContent).toBe("실패");
  expect(res(2).classList.contains("ow-tl__ok")).toBe(true);
  expect(res(2).textContent).toBe("성공추정");
  // 이력이라 경과 시간은 붙이지 않는다
  expect(res(3).classList.contains("ow-tl__run")).toBe(true);
  expect(res(3).textContent).toBe("백그라운드 시작");
  expect(res(4).textContent).toBe("결과 확인 불가");
  expect(res(0).textContent).toBe(""); // 호출 칸은 결과색이 없다
  expect(head(1).querySelector(".ow-tool")!.textContent).toBe("결과");
  // 테스트 수가 있는 실패는 합격·반려 수까지, 성공은 그대로
  expect(res(6).classList.contains("ow-tl__bad")).toBe(true);
  expect(res(6).textContent).toBe("실패 · 합격 11 / 반려 1");
  expect(res(7).textContent).toBe("성공");
  expect(res(10).textContent).toBe("반려");
  // 도구가 아닌 사건의 칩은 한글 종류 이름
  const chip = (i: number) => head(i).querySelector(".ow-tool")!.textContent;
  expect([chip(5), chip(8), chip(9)]).toEqual(["지시", "업무 시작", "PR 등록"]);

  head(0).click();
  expect(head(0).getAttribute("aria-expanded")).toBe("true");
  expect(rows[0].classList.contains("is-open")).toBe(true);
  const open0 = rows[0].querySelector<HTMLElement>(".ow-tl__raw")!;
  expect(open0.hidden).toBe(false);
  expect(open0.textContent).toContain("입력");
  expect(open0.textContent).toContain('{ "command": "cargo test" }');
  expect(open0.querySelector(".ow-emp__cut")).toBeNull();

  head(1).click();
  const body1 = rows[1].querySelector<HTMLElement>(".ow-tl__raw")!;
  expect(body1.textContent).toContain("결과");
  // 가운데 잘려 나간 바이트: 3200 - len("running 12 tests") - len("test result: FAILED")
  expect(body1.querySelector(".ow-emp__cut")!.textContent).toBe(
    "가운데 3.1KB 생략 · 앞뒤 2KB만 표시",
  );
  expect(body1.textContent).toContain("running 12 tests");
  expect(body1.textContent).toContain("test result: FAILED");
  expect(body1.querySelector(".ow-emp__linkfile")!.textContent).toBe("agent-a.jsonl:201");

  head(1).click();
  expect(head(1).getAttribute("aria-expanded")).toBe("false");
  expect(body1.hidden).toBe(true);
  expect(rows[1].classList.contains("is-open")).toBe(false);
});

test("원본 열기는 SourceLine, 사이드카가 있으면 Sidecar 로 open_path 를 부르고 실패하면 문구", async () => {
  let fail = false;
  const s = setup({
    timeline: () => [event(0, { sidecar: "/x/tool-results/big.txt" })],
    open: () => (fail ? reject("open_failed") : undefined),
  });
  await flush();
  s.q<HTMLElement>(".ow-tl__head")!.click();
  const links = s.qa(".ow-emp__link");
  expect(links.map((l) => l.querySelector(".ow-emp__linkfile")!.textContent)).toEqual([
    "agent-a.jsonl:200",
    "big.txt",
  ]);
  links[0].click();
  links[1].click();
  await flush();
  const base = {
    officeId: "o1",
    teamId: "t1",
    agentId: "a",
    cursor: { generation: 1, byteOffset: 1000 },
    lineIdx: 1,
  };
  expect(s.calls.filter(([c]) => c === "open_path").map(([, a]) => a)).toEqual([
    { target: { kind: "SourceLine", ...base } },
    { target: { kind: "Sidecar", ...base } },
  ]);
  fail = true;
  links[0].click();
  await flush();
  const err = s.q<HTMLElement>(".ow-emp__open .ow-pop__error")!;
  expect(err.hidden).toBe(false);
  expect(err.textContent).toBe(phrases.detailError.open_failed);
});

test("더 보기는 마지막 칸의 cursor 를 before 로 50개씩 이어 붙이고, 덜 차면 버튼이 사라진다", async () => {
  const s = setup({
    timeline: (args) => (args.before === null ? page(0, 50) : page(50, 7)),
  });
  await flush();
  expect(s.qa(".ow-tl__row")).toHaveLength(50);
  expect(s.q(".ow-pop__tlhead .ow-pop__n")!.textContent).toBe("50개");
  const more = s.q<HTMLButtonElement>(".ow-pop__more")!;
  expect(more.hidden).toBe(false);
  expect(more.textContent).toBe("더 보기50개씩");
  s.q<HTMLElement>(".ow-tl__head")!.click(); // 펼친 행은 더 보기 뒤에도 그대로
  more.click();
  expect(s.calls[s.calls.length - 1]).toEqual([
    "get_employee_timeline",
    {
      officeId: "o1",
      teamId: "t1",
      agentId: "a",
      before: { generation: 1, byteOffset: 1000 - 49 },
      limit: 50,
    },
  ]);
  await flush();
  expect(s.qa(".ow-tl__row")).toHaveLength(57);
  expect(s.q(".ow-tl__head")!.getAttribute("aria-expanded")).toBe("true");
  expect(s.q(".ow-pop__tlhead .ow-pop__n")!.textContent).toBe("57개");
  expect(s.q<HTMLButtonElement>(".ow-pop__more")!.hidden).toBe(true);
});

test("cursor_invalid 면 처음부터 다시 불러와 목록을 바꾸고 그 문구를 보인다", async () => {
  let more = 0;
  const s = setup({
    timeline: (args) => {
      if (args.before === null) return more === 0 ? page(0, 50) : page(100, 50);
      more++;
      return reject("cursor_invalid");
    },
  });
  await flush();
  s.q<HTMLButtonElement>(".ow-pop__more")!.click();
  await flush();
  const timeline = s.calls.filter(([c]) => c === "get_employee_timeline");
  expect(timeline.map(([, a]) => a!.before)).toEqual([
    null,
    { generation: 1, byteOffset: 1000 - 49 },
    null,
  ]);
  expect(s.qa(".ow-tl__row")).toHaveLength(50);
  expect(s.q(".ow-tl__summary")!.textContent).toBe("src/a100.ts 수정");
  const notice = s.q(".ow-emp__main .ow-pop__error")!;
  expect(notice.hidden).toBe(false);
  expect(notice.textContent).toBe(phrases.detailError.cursor_invalid);
});

test("타임라인만 거절되면 요약은 그대로 두고 코드별 문구, 더 보기 거절은 목록을 지키고 다시 누를 수 있다", async () => {
  let s = setup({ timeline: () => reject("host_offline") });
  await flush();
  expect(s.q(".ow-emp__aside")).not.toBeNull();
  expect(s.q(".ow-emp__main .ow-pop__error")!.textContent).toBe(phrases.detailError.host_offline);
  document.body.innerHTML = "";

  let fail = true;
  s = setup({
    timeline: (args) =>
      args.before === null ? page(0, 50) : fail ? reject("timeout") : page(50, 1),
  });
  await flush();
  const more = s.q<HTMLButtonElement>(".ow-pop__more")!;
  more.click();
  await flush();
  expect(s.qa(".ow-tl__row")).toHaveLength(50);
  expect(s.q(".ow-emp__main .ow-pop__error")!.textContent).toBe(phrases.detailError.timeout);
  expect(more.disabled).toBe(false);
  fail = false;
  more.click();
  await flush();
  expect(s.qa(".ow-tl__row")).toHaveLength(51);
  expect(s.q<HTMLElement>(".ow-emp__main .ow-pop__error")!.hidden).toBe(true);
});

test("같은 agentId 라도 팀이 다르면 그 teamId 로 연다", async () => {
  const s = setup({}, "t2");
  expect(s.calls.map(([, a]) => a!.teamId)).toEqual(["t2", "t2"]);
  await flush();
  // live 가 (t2, a) 를 모르면 머리 이름은 요약 응답으로 채운다
  expect(s.q("#ed-title")!.textContent).toBe("박서준");
});

test("처음부터 다시 불러오기마저 실패해도 더 보기는 마지막 성공한 cursor 를 간직한다", async () => {
  let more = 0;
  const s = setup({
    timeline: (args) => {
      if (args.before === null) return more === 0 ? page(0, 50) : reject("timeout");
      more++;
      return reject("cursor_invalid");
    },
  });
  await flush();
  const btn = s.q<HTMLButtonElement>(".ow-pop__more")!;
  btn.click();
  await flush();
  expect(s.qa(".ow-tl__row")).toHaveLength(50);
  expect(btn.hidden).toBe(false);
  expect(btn.disabled).toBe(false);
  btn.click();
  expect(s.calls[s.calls.length - 1][1]!.before).toEqual({ generation: 1, byteOffset: 1000 - 49 });
});

test("근무·일 마침·개별 퇴근·팀 퇴근을 구별하고 기록만 있는 직원은 확인된 상태만 표시한다", async () => {
  const ended = new Date(2026, 0, 1, 14, 30).toISOString();
  for (const [handlers, text, hidden] of [
    [{ live: emp("a", { state: "Finished", retiredAt: ended }) }, phrases.card.done, false],
    [{ live: emp("a", { state: "Off", departedAt: ended }) }, phrases.sidebar.stateOff, false],
    [{ off: true }, phrases.sidebar.stateOff, false],
    [{ live: null, detail: () => ({ ...detail, retiredAt: ended }) }, phrases.card.done, false],
    [{ live: null }, "", true],
  ] as [Handlers, string, boolean][]) {
    const s = setup(handlers);
    await flush();
    const work = s.q<HTMLElement>(".ow-emp__work")!;
    expect(work.hidden).toBe(hidden);
    if (!hidden) expect(work.textContent).toBe(text);
    expect(work.classList.contains("is-live")).toBe(false);
  }
});

test.each([
  ["Waiting", phrases.sidebar.stateWaiting],
  ["Dozing", phrases.sidebar.stateSleepy],
  ["Incident", phrases.sidebar.stateIncident],
  ["Interrupted", "중단"],
] as const)("실시간 직원 %s 상태를 근무 칩에 표시한다", async (state, label) => {
  const s = setup({ live: emp("a", { state }) });
  await flush();
  const work = s.q<HTMLElement>(".ow-emp__work")!;
  expect(work.textContent).toBe(label);
  expect(work.classList.contains("is-live")).toBe(false);
});

test("머리의 원본 폴더 열기는 등록된 팀 SourceFolder를 쓰고 실패 뒤 버튼을 복구한다", async () => {
  let rejectOpen!: (reason: string) => void;
  const s = setup({ open: () => new Promise((_, reject) => (rejectOpen = reject)) });
  await flush();
  const source = s.q<HTMLButtonElement>(".ow-pop__actions .ow-btn")!;
  expect(source.textContent).toBe(phrases.card.openSource);
  source.click();
  expect(source.disabled).toBe(true);
  expect(s.calls[s.calls.length - 1]).toEqual([
    "open_path",
    { target: { kind: "SourceFolder", officeId: "o1", teamId: "t1" } },
  ]);
  rejectOpen("open_failed");
  await flush();
  expect(source.disabled).toBe(false);
  const error = s.q<HTMLElement>(".ow-pop__head [role='alert']")!;
  expect(error.hidden).toBe(false);
  expect(error.textContent).toBe(phrases.detailError.open_failed);
});

test("키보드로 펼치고 접어도 원문은 실행되지 않으며 Escape 뒤 연 버튼으로 초점이 돌아온다", async () => {
  const user = userEvent.setup();
  const raw = '<img src=x onerror="alert(1)"><script>alert(1)</script>';
  const s = setup({
    timeline: () => [event(0, { body: { head: raw, tail: "", bytes: raw.length } })],
  });
  await flush();
  const head = s.q<HTMLButtonElement>(".ow-tl__head")!;
  head.focus();
  await user.keyboard("{Enter}");
  expect(head.getAttribute("aria-expanded")).toBe("true");
  expect(s.q(".ow-tl__raw pre")!.textContent).toBe(raw);
  expect(s.q(".ow-tl__raw img, .ow-tl__raw script")).toBeNull();
  await user.tab();
  expect(document.activeElement).toBe(s.q(".ow-emp__link"));
  head.focus();
  await user.keyboard("{Enter}");
  expect(head.getAttribute("aria-expanded")).toBe("false");
  await user.tab();
  expect(document.activeElement).toBe(s.q(".ow-pop__actions .ow-btn"));
  await user.keyboard("{Escape}");
  expect(s.q(".ow-modal")).toBeNull();
  expect(document.activeElement).toBe(s.opener);
});

test("닫힌 직원 팝업의 늦은 응답은 팝업을 다시 그리거나 초점을 바꾸지 않는다", async () => {
  let resolveDetail!: (value: EmployeeDetailDto) => void;
  let resolveTimeline!: (value: EventDto[]) => void;
  const s = setup({
    detail: () => new Promise((resolve) => (resolveDetail = resolve)),
    timeline: () => new Promise((resolve) => (resolveTimeline = resolve)),
  });
  document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
  resolveDetail(detail);
  resolveTimeline(page(0, 3));
  await flush();
  expect(s.q(".ow-modal")).toBeNull();
  expect(document.activeElement).toBe(s.opener);
});

test("직원 상세는 원문 라벨을 보존하고 좁은 창 규칙에서 본문과 머리 액션을 다음 줄로 배치한다", async () => {
  const { readFileSync } = await vi.importActual<{
    readFileSync: (path: string, encoding: "utf8") => string;
  }>("node:fs");
  const style = document.createElement("style");
  const narrow = document.createElement("style");
  style.textContent =
    readFileSync("design/handoff/tokens.css", "utf8") +
    readFileSync("src/styles/popup.css", "utf8");
  document.head.append(style);
  try {
    const s = setup();
    await flush();
    const css = (selector: string) => getComputedStyle(s.q(selector)!);
    expect(css(".ow-emp .ow-tl").display).toBe("flex");
    expect(css(".ow-emp .ow-tl").flexDirection).toBe("column");
    expect(css(".ow-tl__row").height).toBe("38px");
    expect(css(".ow-avatar--xl").width).toBe("80px");
    expect(css(".ow-avatar--xl").height).toBe("88px");
    expect(css(".ow-pop__head").paddingLeft).toBe("20px");
    expect(css(".ow-emp__main").paddingLeft).toBe("20px");
    expect(css(".ow-pop__actions .ow-btn").height).toBe("28px");
    s.q<HTMLElement>(".ow-tl__head")!.click();
    expect(css(".ow-tl__row").height).toBe("auto");
    expect(css(".ow-tl__raw").display).toBe("flex");
    expect(s.q(".ow-emp__linklabel")!.textContent).toBe(phrases.employeeDetail.openRaw);
    expect(css(".ow-emp__linklabel").flexShrink).toBe("0");
    expect(css(".ow-emp__linklabel").whiteSpace).toBe("nowrap");
    expect(css(".ow-emp__linkfile").minWidth).toBe("0px");
    expect(css(".ow-emp__linkfile").overflowWrap).toBe("anywhere");
    expect(css(".ow-emp").flexDirection).toBe("row");
    expect(css(".ow-emp__aside").width).toBe("280px");
    // jsdom은 화면 너비로 미디어 쿼리를 적용하지 않으므로 실제 좁은 창 규칙을 꺼내 검증한다.
    const narrowRules = [...style.sheet!.cssRules]
      .filter((rule) => rule.type === CSSRule.MEDIA_RULE)
      .map((rule) => rule as CSSMediaRule)
      .filter((rule) => rule.conditionText === "(max-width: 719px)")
      .flatMap((rule) => [...rule.cssRules]) as CSSStyleRule[];
    narrow.textContent = narrowRules.map((rule) => rule.cssText).join("\n");
    document.head.append(narrow);
    expect(css(".ow-emp").flexDirection).toBe("column");
    expect(css(".ow-emp__aside").width).toBe("auto");
    expect(css(".ow-emp__aside").borderLeftWidth).toBe("0px");
    // var()가 있는 테두리도 jsdom 계산값에서 빠져 선언 자체를 확인한다.
    expect(
      narrowRules.find((rule) => rule.selectorText === ".ow-emp__aside")!.style.borderTop,
    ).toBe("1px solid var(--ow-line)");
    expect(css(".ow-pop__head--emp").gridTemplateColumns).toBe("80px minmax(0, 1fr)");
    expect(css(".ow-pop__actions").gridColumn).toBe("1 / -1");
    expect(css(".ow-pop__actions").justifySelf).toBe("end");
    expect(css(".ow-pop__headmain").overflowWrap).toBe("anywhere");
  } finally {
    narrow.remove();
    style.remove();
  }
});
