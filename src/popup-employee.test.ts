// @vitest-environment jsdom
import { afterEach, expect, test, vi } from "vite-plus/test";
import phrases from "../design/handoff/phrases.ko.json";
import { openEmployeeDetail, type EmployeePopupDeps } from "./popup-employee.ts";
import { emp } from "./test-office.ts";
import type { EmployeeDetailDto, EventDto } from "./types/ipc.ts";

afterEach(() => {
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
  filesEdited: ["src/tail_test.rs"],
  commands: ["cargo test", "cargo fmt", "git diff"],
};

type Handlers = {
  detail?: () => unknown;
  timeline?: (args: Record<string, unknown>) => unknown;
  open?: () => unknown;
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
      t === "t1" && id === "a"
        ? emp("a", { contextUsed: 150_000, contextWindow: 200_000 })
        : undefined,
    now: () => new Date(2026, 0, 1, 14, 31, 0).getTime(),
  };
  const opener = document.createElement("button");
  document.body.append(opener);
  opener.focus();
  openEmployeeDetail(
    deps,
    { officeId: "o1", teamId, agentId: "a", officeName: "B동 203호" },
    opener,
  );
  const q = <T extends Element = HTMLElement>(sel: string) => document.querySelector<T>(sel);
  const qa = (sel: string) => [...document.querySelectorAll<HTMLElement>(sel)];
  return { calls, q, qa, opener };
}

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
    "src/tail_test.rs",
    "cargo test",
    "cargo fmt",
    "git diff",
  ]);
  expect(s.qa(".ow-ev")).toHaveLength(3);
  expect(s.q(".ow-pop__tlhead .ow-pop__n")!.textContent).toBe("50개 중 3개");
});

test("부장은 상사 토막이 빠지고, 요약이 거절되면 코드별 문구만 보인다", async () => {
  let s = setup({ detail: () => ({ ...detail, bossName: null, instruction: null }) });
  await flush();
  expect(s.q(".ow-pop__meta")!.textContent).toBe("입사 14:28 · 근속 3분 · B동 203호");
  expect(s.q(".ow-pop__instruction")).toBeNull();
  document.body.innerHTML = "";

  s = setup({ detail: () => reject("not_found") });
  await flush();
  expect(s.q(".ow-pop__error")!.textContent).toBe(phrases.detailError.not_found);
  expect(s.q(".ow-ev")).toBeNull();
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
    ],
  });
  await flush();
  const rows = s.qa(".ow-ev");
  expect(rows).toHaveLength(6);
  const head = (i: number) => rows[i].querySelector<HTMLElement>(".ow-ev__head")!;
  expect(head(0).querySelector(".ow-ev__tool")!.textContent).toBe("Bash");
  expect(head(0).querySelector(".ow-ev__sum .ow-code")!.textContent).toBe("cargo test");
  expect(head(0).querySelector(".ow-ev__t")!.textContent).toBe("14:31:59");
  expect(head(0).getAttribute("aria-expanded")).toBe("false");
  expect(rows[0].querySelector<HTMLElement>(".ow-ev__body")!.hidden).toBe(true);

  const res = (i: number) => head(i).querySelector<HTMLElement>(".ow-ev__res")!;
  expect(res(1).classList.contains("is-bad")).toBe(true);
  expect(res(1).textContent).toBe("실패");
  expect(res(2).classList.contains("is-ok")).toBe(true);
  expect(res(2).textContent).toBe("성공추정");
  expect(res(3).textContent).toBe("진행 중 · 61초");
  expect(res(4).textContent).toBe("결과 확인 불가");
  expect(res(0).textContent).toBe(""); // 호출 칸은 결과색이 없다
  expect(head(1).querySelector(".ow-ev__tool")!.textContent).toBe("ToolResult");

  head(0).click();
  expect(head(0).getAttribute("aria-expanded")).toBe("true");
  const open0 = rows[0].querySelector<HTMLElement>(".ow-ev__body")!;
  expect(open0.hidden).toBe(false);
  expect(open0.textContent).toContain("입력");
  expect(open0.textContent).toContain('{ "command": "cargo test" }');
  expect(open0.querySelector(".ow-emp__cut")).toBeNull();

  head(1).click();
  const body1 = rows[1].querySelector<HTMLElement>(".ow-ev__body")!;
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
});

test("원본 열기는 SourceLine, 사이드카가 있으면 Sidecar 로 open_path 를 부르고 실패하면 문구", async () => {
  let fail = false;
  const s = setup({
    timeline: () => [event(0, { sidecar: "/x/tool-results/big.txt" })],
    open: () => (fail ? reject("open_failed") : undefined),
  });
  await flush();
  s.q<HTMLElement>(".ow-ev__head")!.click();
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
  expect(s.qa(".ow-ev")).toHaveLength(50);
  const more = s.q<HTMLButtonElement>(".ow-pop__more")!;
  expect(more.hidden).toBe(false);
  expect(more.textContent).toBe("더 보기50개씩");
  s.q<HTMLElement>(".ow-ev__head")!.click(); // 펼친 행은 더 보기 뒤에도 그대로
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
  expect(s.qa(".ow-ev")).toHaveLength(57);
  expect(s.q(".ow-ev__head")!.getAttribute("aria-expanded")).toBe("true");
  expect(s.q(".ow-pop__tlhead .ow-pop__n")!.textContent).toBe("57");
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
  expect(s.qa(".ow-ev")).toHaveLength(50);
  expect(s.q(".ow-ev__sum")!.textContent).toBe("src/a100.ts 수정");
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
  expect(s.qa(".ow-ev")).toHaveLength(50);
  expect(s.q(".ow-emp__main .ow-pop__error")!.textContent).toBe(phrases.detailError.timeout);
  expect(more.disabled).toBe(false);
  fail = false;
  more.click();
  await flush();
  expect(s.qa(".ow-ev")).toHaveLength(51);
  expect(s.q<HTMLElement>(".ow-emp__main .ow-pop__error")!.hidden).toBe(true);
});

test("같은 agentId 라도 팀이 다르면 그 teamId 로 연다", async () => {
  const s = setup({}, "t2");
  expect(s.calls.map(([, a]) => a!.teamId)).toEqual(["t2", "t2"]);
  await flush();
  // live 가 (t2, a) 를 모르면 머리 이름은 요약 응답으로 채운다
  expect(s.q("#ed-title")!.textContent).toBe("박서준");
});

test("Esc 로 닫으면 연 요소로 초점이 돌아간다", async () => {
  const s = setup();
  await flush();
  document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
  expect(s.q(".ow-backdrop")).toBeNull();
  expect(document.activeElement).toBe(s.opener);
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
  expect(s.qa(".ow-ev")).toHaveLength(50);
  expect(btn.hidden).toBe(false);
  expect(btn.disabled).toBe(false);
  btn.click();
  expect(s.calls[s.calls.length - 1][1]!.before).toEqual({ generation: 1, byteOffset: 1000 - 49 });
});
