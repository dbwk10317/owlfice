import "../src/styles/app.css";
import { mountHeader } from "../src/header.ts";
import { mountOffice, type OfficeHandle } from "../src/office/renderer.ts";
import { openEmployeeDetail } from "../src/popup-employee.ts";
import type { Invoke } from "../src/scale.ts";
import { mountSidebar, popupNames } from "../src/sidebar-left.ts";
import { mountPanel } from "../src/sidebar-right.ts";
import { createStore } from "../src/store.ts";
import { act, emp, office, team } from "../src/test-office.ts";
import type {
  CueDto,
  EmployeeDetailDto,
  EventDto,
  OfficeDto,
  OfficeListDto,
  OfficeSummaryDto,
  Scale,
  TeamDto,
} from "../src/types/ipc.ts";

const since = new Date(Date.now() - 60_000).toISOString();
function makeTeam(id: string, name: string, slot: number): TeamDto {
  const t = team(
    id,
    [
      emp("boss", {
        name,
        rank: "Bujang",
        hiredAt: since,
        activity: act({ room: "TeamLead", pose: "Type", since, sentence: "작업 계획 작성 중" }),
      }),
      emp("sub", {
        name: `${name}동료`,
        parent: "boss",
        hiredAt: since,
        seat: slot,
        activity: act({ room: "Dev", pose: "Type", since, sentence: "화면 수정 중" }),
      }),
    ],
    { title: `${name} 업무`, deskSlot: slot, started: since, lastActivity: since, building: "B" },
  );
  t.props.safeSince = since;
  t.props.kanban = [{ content: `${name} 작업`, status: slot === 0 ? "Pending" : "Done" }];
  return t;
}
const teams = [makeTeam("one", "한결", 0), makeTeam("two", "다온", 1)];
let scale: Scale = "Small";
let sidebarStates = false;
let seq = 1;
let selectRequest = 0;
const store = createStore<OfficeListDto, OfficeDto>();
const companyName = "야근불 연구소";
const app = document.getElementById("app")!;
const side = document.getElementById("side")!;
const stage = document.getElementById("stage")!;
const panelHost = document.getElementById("panel")!;
const status = document.getElementById("status")!;

function summary(id: string, members: TeamDto[], floor: number): OfficeSummaryDto {
  const active = members.some((t) => t.phase !== "Off");
  const asking = sidebarStates && members.some((t) => t.teamId === "one" && t.phase !== "Off");
  return {
    officeId: id,
    building: scale === "Large" ? "B" : null,
    floor,
    room: scale === "Large" ? floor + 100 : null,
    label: scale === "Small" ? "1층 Claude 사무실" : `B동 ${floor + 100}호`,
    phase: active ? "Working" : "Off",
    dot: active ? (sidebarStates ? (asking ? "Waiting" : "IdleAlive") : "Working") : "Off",
    asking,
    border: "Plain",
    delayed: false,
    teams: members.map((t) => ({
      teamId: t.teamId,
      bossName: t.employees[0].name,
      title: t.title,
      folder: t.folder,
      others: 1,
      started: since,
      lastActivity: sidebarStates
        ? new Date(Date.now() - 35 * 60_000).toISOString()
        : t.lastActivity,
      readFailed: sidebarStates && t.teamId === "one",
      phase: t.phase,
      dot:
        t.phase === "Off"
          ? "Off"
          : sidebarStates
            ? t.teamId === "one"
              ? "Waiting"
              : "IdleAlive"
            : "Working",
      asking: sidebarStates && t.teamId === "one" && t.phase !== "Off",
      askingCount: sidebarStates && t.teamId === "one" && t.phase !== "Off" ? 2 : 0,
      border: "Plain",
      delayed: false,
      hostBadge: sidebarStates && t.teamId === "two" ? "WSL" : null,
      workType: t.workType,
    })),
  };
}

function snapshot(id: string, rebuilt = false, cues: CueDto[] = []): OfficeDto {
  const members = scale === "Small" ? teams : teams.filter((t) => t.teamId === id);
  const latest = members.reduce<TeamDto | undefined>(
    (a, t) => (!a || t.lastActivity >= a.lastActivity ? t : a),
    undefined,
  );
  const o = office(id, seq, structuredClone(members), cues);
  o.rebuilt = rebuilt;
  o.shared.frameTitle = latest?.title ?? "";
  o.shared.kanban = latest?.props.kanban ?? [];
  o.shared.safeSince = since;
  return o;
}

function listing(id: string, rebuilt = false): OfficeListDto {
  return {
    listSeq: seq,
    rebuilt,
    scale,
    selectedId: id,
    selectRequest,
    offices:
      scale === "Small"
        ? [summary("vendor/Claude", teams, 1)]
        : teams.map((t, i) => summary(t.teamId, [t], i + 1)),
  };
}

function publish(
  cues: CueDto[] = [],
  rebuilt = false,
  id = store.state.selectedId ?? "vendor/Claude",
): void {
  seq++;
  store.applyList(listing(id, rebuilt));
  store.applyOfficeState(snapshot(id, rebuilt, cues));
  status.textContent = `${scale === "Small" ? "중소기업" : "대기업"} · ${teams.filter((t) => t.phase !== "Off").length}팀 근무`;
}

function openEmployee(
  officeId: string,
  teamId: string,
  agentId: string,
  opener?: HTMLElement,
): void {
  const selectedTeam = store.state.office?.teams.find((t) => t.teamId === teamId);
  const employee = selectedTeam?.employees.find((e) => e.agentId === agentId);
  if (!selectedTeam || !employee) return;
  // README용 합성 기록이다. 파일 이름과 명령은 표시할 문자열이며 읽거나 실행하지 않는다.
  const detail: EmployeeDetailDto = {
    agentId: employee.agentId,
    name: employee.name,
    rank: employee.rank,
    role: employee.role,
    model: employee.model,
    hiredAt: employee.hiredAt,
    retiredAt: employee.retiredAt,
    bossName: selectedTeam.employees.find((e) => e.agentId === employee.parent)?.name ?? null,
    instruction: "[합성 예시] 설정 화면의 간격을 다듬고 테스트를 확인해 주세요.",
    filesRead: ["src/app.ts"],
    filesEdited: [{ item: "src/app.ts", times: 1 }],
    commands: [{ item: "npm test", times: 1 }],
  };
  const events: EventDto[] = [
    { tool: "Bash", summary: "`npm test` 성공 · 합격 12 / 반려 0", input: "npm test" },
    { tool: "Edit", summary: "`src/app.ts` 설정 화면 간격 수정", input: "src/app.ts · 간격 16px" },
    { tool: "Read", summary: "`src/app.ts` 읽기", input: "src/app.ts" },
  ].map((sample, i) => {
    const input = `[합성 예시] ${sample.input}`;
    const body = "[합성 예시] Tests 12 passed (12)";
    return {
      cursor: { generation: 1, byteOffset: (3 - i) * 100 },
      agentId: employee.agentId,
      ts: new Date(Date.parse(since) + (3 - i) * 10_000).toISOString(),
      kind: i === 0 ? "ToolResult" : "ToolCall",
      summary: sample.summary,
      confidence: "Confirmed",
      tool: sample.tool,
      callId: `synthetic-call-${i}`,
      input: { head: input, tail: "", bytes: new TextEncoder().encode(input).length },
      outcome: i === 0 ? "Ok" : null,
      body: i === 0 ? { head: body, tail: "", bytes: new TextEncoder().encode(body).length } : null,
      path: `synthetic/README/${teamId}-${agentId}.jsonl`,
      lineNo: 3 - i,
      lineIdx: 0,
      sidecar: null,
      test: i === 0 ? { passed: 12, failed: 0 } : null,
    };
  });
  const invoke: Invoke = async <T>(cmd: string, args?: Record<string, unknown>): Promise<T> => {
    if (cmd === "open_path") throw new Error("합성 미리보기에서는 원본 파일을 열지 않습니다.");
    if (args?.officeId !== officeId || args.teamId !== teamId || args.agentId !== agentId)
      throw new Error("합성 직원 정보가 일치하지 않습니다.");
    if (cmd === "get_employee_detail") return detail as T;
    if (cmd === "get_employee_timeline") return (args.before === null ? events : []) as T;
    throw new Error(`합성 응답이 없는 명령: ${cmd}`);
  };
  openEmployeeDetail(
    {
      invoke,
      live: (id, agent) =>
        store.state.office?.teams
          .find((t) => t.teamId === id)
          ?.employees.find((e) => e.agentId === agent),
    },
    {
      officeId,
      teamId,
      agentId,
      officeName: popupNames(store.state.list, officeId, teamId).officeName,
      building: selectedTeam.building,
    },
    opener,
  );
}

store.applyWorld({ ...listing("vendor/Claude"), selected: snapshot("vendor/Claude") });
let header: ReturnType<typeof mountHeader> | undefined;
let view: OfficeHandle | undefined;
let pendingTeam: { officeId: string; teamId: string } | null = null;
const panel = mountPanel(panelHost, store, {
  onDrawerChange: (open) => header?.setDrawerOpen(open),
  onEmployee: (teamId, agentId, opener) => {
    const officeId = store.state.selectedId;
    if (officeId) openEmployee(officeId, teamId, agentId, opener);
  },
});
mountSidebar(
  side,
  store,
  (id, requestId) => {
    selectRequest = requestId;
    seq++;
    store.applySelectResponse({ ...snapshot(id), requestId });
    store.applyList(listing(id));
  },
  {
    companyName,
    onTeam: (officeId, teamId) => {
      panel.showTeam(officeId, teamId);
      if (view) view.highlightTeam(officeId, teamId);
      else pendingTeam = { officeId, teamId };
    },
  },
);
header = mountHeader(app, document.getElementById("header")!, store, {
  companyName,
  bandSlot: () => side.querySelector(".ow-side-band"),
  togglePanel: () => panel.toggleDrawer(),
  setRoomLabels: (on) => view?.setRoomLabels(on),
  setScale: (scale) => view?.setScale(scale),
  stage,
  centerCanvas: () => view?.centerCanvas(),
});
await mountOffice(stage, store, {
  onScale: (n, maxFit) => header?.setScale(n, maxFit),
  onEmployee: (officeId, teamId, agentId) => {
    panel.highlight(teamId, agentId);
    openEmployee(officeId, teamId, agentId);
  },
}).then((handle) => {
  view = handle;
  if (pendingTeam) view.highlightTeam(pendingTeam.officeId, pendingTeam.teamId);
  pendingTeam = null;
  view.setRoomLabels(header?.roomLabels() ?? true);
  app.dataset.ready = "true";
});

document.getElementById("arrive")!.addEventListener("click", () => {
  let arriving = teams.find((t) => t.teamId === "three");
  if (arriving?.phase === "Working") return;
  if (!arriving) {
    arriving = makeTeam("three", "라온", 2);
    teams.push(arriving);
  }
  arriving.phase = "Working";
  arriving.lastActivity = new Date().toISOString();
  arriving.deskSlot = scale === "Small" ? 2 : 0;
  arriving.employees[1].seat = scale === "Small" ? 2 : 0;
  publish(arriving.employees.map((e) => ({ teamId: "three", kind: "Enter", agent: e.agentId })));
});
document.getElementById("depart")!.addEventListener("click", () => {
  const leaving = teams.find((t) => t.teamId === "three");
  if (!leaving || leaving.phase === "Off") return;
  leaving.phase = "Off";
  leaving.deskSlot = null;
  leaving.employees[1].seat = null;
  publish(leaving.employees.map((e) => ({ teamId: "three", kind: "Depart", agent: e.agentId })));
});
document.getElementById("scale")!.addEventListener("click", () => {
  scale = scale === "Small" ? "Large" : "Small";
  let slot = 0;
  for (const t of teams) {
    t.deskSlot = t.phase === "Off" ? null : scale === "Small" ? slot++ : 0;
    t.employees[1].seat = t.deskSlot;
  }
  const latest = teams.reduce((a, t) => (t.lastActivity >= a.lastActivity ? t : a));
  publish([], true, scale === "Small" ? "vendor/Claude" : latest.teamId);
});
document.getElementById("parallel")!.addEventListener("click", () => {
  const worker = teams[0].employees[1];
  worker.activity.extra = worker.activity.extra === 0 ? 2 : 0;
  worker.activity.sentence = worker.activity.extra ? "화면 수정 중 (외 2건)" : "화면 수정 중";
  publish();
});
document.getElementById("visit")!.addEventListener("click", () => {
  teams[1].props.bossVisit = "다온 팀의 수정 결과를 확인해 주세요";
  publish([{ teamId: "two", kind: "BossVisit", text: teams[1].props.bossVisit }]);
});
document.getElementById("sidebar-states")!.addEventListener("click", () => {
  sidebarStates = !sidebarStates;
  publish();
});
publish();
