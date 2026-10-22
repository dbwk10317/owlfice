// 개발용 사장님 연출 확인 페이지 (npm run dev 에서 /dev/boss.html). 앱 빌드에 들어가지 않는다.
// 버튼이 부장 자세와 cue 를 바꿔 방문·응답 대기·러버덕·결재 대기·허가 대기·반려를 보여 준다.
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

const since = new Date().toISOString();

function boss(room: Room, pose: Pose): EmployeeDto {
  return {
    agentId: "boss",
    name: "김부장",
    rank: "Bujang",
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
    buildTestBorder: "Plain",
    lastSay: null,
    hiredAt: since,
    retiredAt: null,
    departedAt: null,
    seat: null,
    parent: null,
    depth: 1,
    confidence: "Confirmed",
    contextUsed: null,
    contextWindow: null,
    bubble: null,
    jobs: 0,
    meeting: null,
    review: null,
  };
}

let seq = 1;
function office(room: Room, pose: Pose, cues: CueDto[] = []): OfficeDto {
  const t = testTeam("t1", [boss(room, pose)]);
  const o = testOffice(
    "dev-boss",
    seq++,
    [{ ...t, title: "t1", folder: "app", started: since, lastActivity: since, deskSlot: 0 }],
    cues,
  );
  return {
    ...o,
    shared: {
      ...o.shared,
      incident: false,
      night: false,
      light: "Green",
      safeSince: since,
      trash: 0,
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
  selectedId: "dev-boss",
  selectRequest: 0,
};
const listeners = new Set<() => void>();
const store = {
  state: { list, selectedId: "dev-boss", office: office("TeamLead", "Sit") },
  subscribe(fn: () => void) {
    listeners.add(fn);
    return () => listeners.delete(fn);
  },
};
const set = (room: Room, pose: Pose, cues: CueDto[] = []) => {
  store.state.office = office(room, pose, cues);
  for (const fn of listeners) fn();
};

const STEPS: [string, () => void][] = [
  ["앉아 있음", () => set("TeamLead", "Sit")],
  [
    "사장님 방문 (S03)",
    () =>
      set("TeamLead", "Sit", [{ teamId: "t1", kind: "BossVisit", text: "배포 상황 보고하세요" }]),
  ],
  ["응답 대기 (S04)", () => set("TeamLead", "Think")],
  ["응답 대기 10초+ (S05)", () => set("TeamLead", "Duck")],
  ["결재 대기 (S32)", () => set("TeamLead", "Pace")],
  ["허가 대기 (S34)", () => set("Door", "Sit")],
  ["반려 (S49)", () => set("TeamLead", "Sit", [{ teamId: "t1", kind: "Reject", agent: "boss" }])],
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
