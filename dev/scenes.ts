// 개발용 미팅실·외근·사고·장애·리뷰실 연출 확인 페이지 (npm run dev 에서 /dev/scenes.html). 앱 빌드에 들어가지 않는다.
// 버튼이 직원 방·문장과 공유 소품(장애)을 바꿔 S21~S28 을 보여 준다. 외근 복귀처럼 "어디서 어디로" 가는 장면은
// 앞 상태를 먼저 보낸 뒤 다음 상태를 보낸다.
import { mountOffice } from "../src/office/renderer.ts";
import { office as testOffice, team as testTeam } from "../src/test-office.ts";
import type {
  CueDto,
  EmployeeDto,
  OfficeDto,
  OfficeListDto,
  Pose,
  Room,
} from "../src/types/ipc.ts";

const T = "t1";
const iso = (msAgo = 0) => new Date(Date.now() - msAgo).toISOString();

function emp(
  agentId: string,
  rank: "Bujang" | "Daeri",
  room: Room,
  pose: Pose,
  sentence = `${room} ${pose}`,
) {
  const since = iso();
  return {
    agentId,
    name: rank === "Bujang" ? "김부장" : "박대리",
    rank,
    role: "Backend",
    model: null,
    description: null,
    activity: {
      sentence,
      room,
      since,
      expectedSecs: null,
      pose,
      confidence: "Confirmed",
      extra: 0,
      elapsed: false,
    },
    recent: [],
    counters: { read: 0, edit: 0, shell: 0, outside: 0, meeting: 0, errors: 0 },
    uniqueReads: 0,
    pendingCount: 0,
    uniqueEdits: 0,
    planning: false,
    medal: false,
    buildTestBorder: "Plain",
    lastSay: null,
    hiredAt: since,
    retiredAt: null,
    departedAt: null,
    seat: rank === "Bujang" ? null : 0,
    parent: null,
    depth: 1,
    confidence: "Confirmed",
    contextUsed: null,
    contextWindow: null,
    bubble: null,
    jobs: 0,
    meeting: null,
    review: null,
  } satisfies EmployeeDto;
}

let seq = 1;
function office(
  boss: EmployeeDto,
  worker: EmployeeDto,
  incident = false,
  trash = 0,
  cues: CueDto[] = [],
): OfficeDto {
  const since = iso();
  const t = testTeam(T, [boss, worker]);
  const o = testOffice(
    "dev-scenes",
    seq++,
    [{ ...t, title: T, folder: "app", started: since, lastActivity: since, deskSlot: 0 }],
    cues,
  );
  return {
    ...o,
    shared: {
      ...o.shared,
      incident,
      night: false,
      light: "Off",
      safeSince: since,
      trash,
      pizza: 0,
      kanban: [],
    },
  };
}

const list: OfficeListDto = {
  listSeq: 1,
  rebuilt: false,
  scale: "Large",
  offices: [],
  selectedId: "dev-scenes",
  selectRequest: 0,
};
const lead = () => emp("boss", "Bujang", "TeamLead", "Sit");
const desk = () => emp("a", "Daeri", "Dev", "Type");
const listeners = new Set<() => void>();
const store = {
  state: { list, selectedId: "dev-scenes", office: office(lead(), desk()) },
  subscribe(fn: () => void) {
    listeners.add(fn);
    return () => listeners.delete(fn);
  },
};
const show = (o: OfficeDto) => {
  store.state.office = o;
  for (const fn of listeners) fn();
};
const worker = (w: EmployeeDto, incident = false, trash = 0, cues: CueDto[] = []) =>
  show(office(lead(), w, incident, trash, cues));
const cue = (c: Record<string, unknown>) => ({ teamId: T, ...c }) as CueDto;
const then = (first: () => void, next: () => void) => {
  first();
  setTimeout(next, 150);
};

const mcp: EmployeeDto = {
  ...emp("a", "Daeri", "Meeting", "Sit", "거래처 `figma`와 `get_file` 건으로 미팅 중"),
  meeting: { kind: "Mcp", server: "figma" },
};
const skill: EmployeeDto = {
  ...emp("a", "Daeri", "Meeting", "Sit", "외주 매뉴얼 `pdf` 펼쳐보는 중"),
  meeting: { kind: "Skill" },
};
const away = emp("a", "Daeri", "Outside", "Sit", "'검색어' 건으로 외근 중");
const review = (n: number): EmployeeDto => ({
  ...emp("a", "Daeri", "Review", "Sit", `PR 리뷰 중 · 빨간펜 ${n}개`),
  review: { pens: n, approved: false },
});

const STEPS: [string, () => void][] = [
  ["자리에 앉음", () => worker(desk())],
  ["MCP 미팅 (S21)", () => worker(mcp)],
  ["스킬 사용 (S22)", () => worker(skill)],
  ["웹 검색, 외근 (S23)", () => worker(away)],
  [
    "웹 복귀 (S24)",
    () =>
      then(
        () => worker(away),
        () => worker(desk()),
      ),
  ],
  ["도구 에러 1건 (S25)", () => worker(desk(), false, 1, [cue({ kind: "Shake", agent: "a" })])],
  [
    "장애 판정 (S26)",
    () =>
      show(
        office(
          emp("boss", "Bujang", "Incident", "Sit"),
          emp("a", "Daeri", "Incident", "Sit"),
          true,
          1,
          [cue({ kind: "Siren", on: true })],
        ),
      ),
  ],
  [
    "장애 해제 (S27)",
    () =>
      then(
        () =>
          show(
            office(
              emp("boss", "Bujang", "Incident", "Sit"),
              emp("a", "Daeri", "Incident", "Sit"),
              true,
            ),
          ),
        () => worker(desk(), false, 1, [cue({ kind: "Siren", on: false })]),
      ),
  ],
  ["리뷰 시작, 지적 0개 (S28)", () => worker(review(0))],
  ["지적 3개", () => worker(review(3))],
  ["지적 9개 (화면 슬롯 6개까지)", () => worker(review(9))],
];
const buttons = document.getElementById("buttons")!;
for (const [label, run] of STEPS) {
  const b = document.createElement("button");
  b.textContent = label;
  b.onclick = run;
  buttons.append(b);
}

mountOffice(document.getElementById("large")!, store).catch((e) => {
  document.body.append(`실패: ${e}`);
  console.error(e);
});
