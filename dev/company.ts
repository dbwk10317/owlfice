import "../src/styles/app.css";
import { mountOffice } from "../src/office/renderer.ts";
import { mountSidebar } from "../src/sidebar-left.ts";
import { mountPanel } from "../src/sidebar-right.ts";
import { createStore } from "../src/store.ts";
import { act, emp, office, team } from "../src/test-office.ts";
import type {
  CueDto,
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
let seq = 1;
let selectRequest = 0;
const store = createStore<OfficeListDto, OfficeDto>();
const app = document.getElementById("app")!;
const side = document.createElement("div");
const stage = document.createElement("div");
stage.className = "ow-stage";
const main = document.createElement("div");
main.className = "ow-main";
main.append(stage);
const panelHost = document.createElement("div");
panelHost.className = "ow-panel-host";
app.append(side, main, panelHost);
const status = document.getElementById("status")!;

function summary(id: string, members: TeamDto[], floor: number): OfficeSummaryDto {
  const active = members.some((t) => t.phase !== "Off");
  return {
    officeId: id,
    building: scale === "Large" ? "B" : null,
    floor,
    room: scale === "Large" ? floor + 100 : null,
    label: scale === "Small" ? "Claude" : members[0].title,
    phase: active ? "Working" : "Off",
    dot: active ? "Working" : "Off",
    asking: false,
    border: "Plain",
    delayed: false,
    teams: members.map((t) => ({
      teamId: t.teamId,
      bossName: t.employees[0].name,
      title: t.title,
      folder: t.folder,
      others: 1,
      started: since,
      phase: t.phase,
      dot: t.phase === "Off" ? "Off" : "Working",
      asking: false,
      border: "Plain",
      delayed: false,
      hostBadge: null,
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

store.applyWorld({ ...listing("vendor/Claude"), selected: snapshot("vendor/Claude") });
const panel = mountPanel(panelHost, store);
const view = await mountOffice(stage, store);
view.setRoomLabels(false);
document.getElementById("labels")!.addEventListener("change", (event) => {
  view.setRoomLabels((event.target as HTMLInputElement).checked);
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
    companyName: "합성 연출실",
    onTeam: (officeId, teamId) => {
      panel.showTeam(officeId, teamId);
      view.highlightTeam(officeId, teamId);
    },
  },
);

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
publish();
