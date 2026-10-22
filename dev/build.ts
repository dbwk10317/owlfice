// 개발용 빌드실·테스트실·출고장 연출 확인 페이지 (npm run dev 에서 /dev/build.html). 앱 빌드에 들어가지 않는다.
// 버튼이 직원 자세와 cue 를 바꿔 셸·빌드·테스트·도장·쪼그림·땀·커밋·트럭·반송·폭죽·작업표를 보여 준다.
// 도장·만세처럼 "어디서 어디로" 가는 장면은 앞 상태를 먼저 보여 준 뒤 cue 와 함께 다음 상태를 보낸다.
import { mountOffice } from "../src/office/renderer.ts";
import { office as testOffice, team as testTeam } from "../src/test-office.ts";
import type {
  CueDto,
  EmployeeDto,
  Light,
  OfficeDto,
  OfficeListDto,
  Pose,
  Room,
  TestVerdict,
} from "../src/types/ipc.ts";

const T = "t1";
const iso = (msAgo = 0) => new Date(Date.now() - msAgo).toISOString();

function emp(agentId: string, rank: "Bujang" | "Daeri", room: Room, pose: Pose, msAgo = 0) {
  const since = iso(msAgo);
  return {
    agentId,
    name: rank === "Bujang" ? "김부장" : "박대리",
    rank,
    role: "Backend",
    model: null,
    description: null,
    activity: {
      sentence: `${room} ${pose}`,
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
  worker: EmployeeDto,
  light: Light,
  cues: CueDto[] = [],
  test: TestVerdict | null = null,
): OfficeDto {
  const since = iso();
  const t = testTeam(T, [emp("boss", "Bujang", "TeamLead", "Sit"), worker]);
  const o = testOffice(
    "dev-build",
    seq++,
    [{ ...t, title: T, folder: "app", started: since, lastActivity: since, deskSlot: 0 }],
    cues,
  );
  return {
    ...o,
    shared: {
      ...o.shared,
      incident: false,
      night: false,
      light,
      safeSince: since,
      trash: 0,
      pizza: 0,
      kanban: [],
      test,
    },
  };
}

const list: OfficeListDto = {
  listSeq: 1,
  rebuilt: false,
  scale: "Large",
  offices: [],
  selectedId: "dev-build",
  selectRequest: 0,
};
const listeners = new Set<() => void>();
const store = {
  state: { list, selectedId: "dev-build", office: office(emp("a", "Daeri", "Dev", "Type"), "Off") },
  subscribe(fn: () => void) {
    listeners.add(fn);
    return () => listeners.delete(fn);
  },
};
const show = (
  worker: EmployeeDto,
  light: Light,
  cues: CueDto[] = [],
  test: TestVerdict | null = null,
) => {
  store.state.office = office(worker, light, cues, test);
  for (const fn of listeners) fn();
};

const dev = (light: Light = "Off") => show(emp("a", "Daeri", "Dev", "Type"), light);
const at = (room: Room, pose: Pose = "Sit", msAgo = 0) => emp("a", "Daeri", room, pose, msAgo);
const cue = (c: Record<string, unknown>) => ({ teamId: T, ...c }) as CueDto;
// 앞 상태를 그리고 이동이 시작되게 한 틱 뒤에 다음 상태를 보낸다
const then = (first: () => void, next: () => void) => {
  first();
  setTimeout(next, 150);
};

const STEPS: [string, () => void][] = [
  ["자리에 앉음", () => dev()],
  ["셸 명령 (S10)", () => show(at("Build"), "Off")],
  ["빌드 중 (S11)", () => show(at("Build"), "Yellow")],
  ["빌드 성공 (S12)", () => show(at("Build"), "Green")],
  ["빌드 실패 (S13)", () => show(at("Build"), "Red")],
  ["테스트 실행 (S14)", () => show(at("Test"), "Yellow")],
  [
    "테스트 전부 통과 (S15)",
    () =>
      then(
        () => show(at("Test"), "Yellow"),
        () =>
          show(
            emp("a", "Daeri", "Dev", "Type"),
            "Green",
            [
              cue({
                kind: "Stamp",
                confirmedPass: true,
                agent: "a",
                at: iso(),
                passed: 12,
                failed: 0,
                estimated: false,
              }),
            ],
            "Pass",
          ),
      ),
  ],
  [
    "테스트 일부 실패 (S16)",
    () =>
      then(
        () => show(at("Test"), "Yellow"),
        () =>
          show(
            emp("a", "Daeri", "Dev", "Type"),
            "Red",
            [
              cue({
                kind: "Stamp",
                confirmedPass: false,
                agent: "a",
                at: iso(),
                passed: 10,
                failed: 2,
                estimated: false,
              }),
            ],
            "Fail",
          ),
      ),
  ],
  [
    "파이프 섞인 테스트, 추정 (S15·S16)",
    () =>
      then(
        () => show(at("Test"), "Yellow"),
        () =>
          show(
            emp("a", "Daeri", "Dev", "Type"),
            "Yellow",
            [
              cue({
                kind: "Stamp",
                confirmedPass: false,
                agent: "a",
                at: iso(),
                passed: 12,
                failed: 0,
                estimated: true,
              }),
            ],
            "Pass",
          ),
      ),
  ],
  ["평균 2배 초과 (S17)", () => show(at("Build", "Squat", 30_000), "Yellow")],
  ["2분 초과 (S18)", () => show(at("Build", "Squat", 130_000), "Yellow")],
  ["커밋 (S29)", () => show(at("Shipping"), "Off")],
  [
    "push 성공 (S30)",
    () =>
      then(
        () => show(at("Shipping"), "Off"),
        () => show(at("Shipping"), "Off", [cue({ kind: "Truck" })]),
      ),
  ],
  [
    "릴리스 (S31)",
    () =>
      then(
        () => show(at("Shipping"), "Off"),
        () => show(at("Shipping"), "Off", [cue({ kind: "Fireworks" })]),
      ),
  ],
  [
    "push 실패, 반송 (S48)",
    () =>
      then(
        () => show(at("Shipping"), "Off"),
        () => show(at("Shipping"), "Off", [cue({ kind: "Return" })]),
      ),
  ],
  ["백그라운드 명령, 작업표 꽂기 (S61)", () => show({ ...at("Dev", "Type"), jobs: 1 }, "Off")],
  ["작업표 빼기", () => show(at("Dev", "Type"), "Off")],
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
