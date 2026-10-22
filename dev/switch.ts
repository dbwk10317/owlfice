// 합성 DTO로 IPC 응답 순서와 실패를 조절한다. 실제 세션이나 Tauri 커맨드는 읽지 않는다.
import "../design/handoff/tokens.css";
import "../src/styles/app.css";
import { mountHeader } from "../src/header.ts";
import { mountOffice, type OfficeHandle } from "../src/office/renderer.ts";
import { mountSidebar } from "../src/sidebar-left.ts";
import { mountPanel } from "../src/sidebar-right.ts";
import { createStore } from "../src/store.ts";
import { act, emp, office, team } from "../src/test-office.ts";
import type { OfficeDto, OfficeListDto, SelectResponseDto, WorldDto } from "../src/types/ipc.ts";

const get = (id: string) => document.getElementById(id)!;
const store = createStore<OfficeListDto, OfficeDto>();
const offices = ["A", "B", "C"].map((id, n) =>
  office(id, 1, [
    team(`team-${id}`, [
      emp(`${id}-boss`, {
        name: `${id} 부장`,
        rank: "Bujang",
        depth: 0,
        activity: act({ room: "TeamLead", pose: "Sit", sentence: `${id} 사무실 지휘 중` }),
      }),
      emp(`${id}-staff`, {
        name: `${id} 직원`,
        parent: `${id}-boss`,
        seat: n,
        activity: act({
          room: n === 1 ? "Archive" : "Dev",
          pose: n === 1 ? "Read" : "Type",
          sentence: `${id} 사무실 작업 중`,
        }),
      }),
    ]),
  ]),
);
let backend: WorldDto = {
  listSeq: 1,
  rebuilt: false,
  scale: "Large",
  selectedId: "A",
  selectRequest: 0,
  selected: offices[0],
  offices: offices.map((o, n) => ({
    officeId: o.officeId,
    building: "A",
    floor: 1,
    room: 101 + n,
    label: `${o.officeId} 사무실`,
    phase: "Working",
    dot: "Working",
    asking: false,
    border: "Plain",
    delayed: false,
    teams: [
      {
        teamId: `team-${o.officeId}`,
        bossName: `${o.officeId} 부장`,
        title: `${o.officeId} 팀`,
        folder: `sample-${o.officeId}`,
        others: 1,
        started: new Date().toISOString(),
        lastActivity: new Date().toISOString(),
        readFailed: false,
        phase: "Working",
        dot: "Working",
        asking: false,
        askingCount: 0,
        border: "Plain",
        delayed: false,
        hostBadge: null,
        workType: "NewDev",
      },
    ],
  })),
};
let requests = 0;
let snapshots = 0;
let failSnapshot = false;

function status(): void {
  const state = store.state;
  get("status").textContent =
    `선택 ${state.selectedId} · 표시 ${state.office?.officeId ?? "대기"} · ` +
    `백엔드 ${backend.selectedId} · 선택 요청 ${requests}회 · 복구 스냅샷 ${snapshots}회`;
  const error = get("error");
  error.hidden = state.selectionError === null;
  error.textContent =
    state.selectionError === "recovering"
      ? "사무실을 전환하지 못했습니다. 현재 사무실을 다시 불러오는 중입니다."
      : state.selectionError === "restored"
        ? "사무실을 전환하지 못해 현재 사무실을 다시 불러왔습니다."
        : state.selectionError === "failed"
          ? "사무실을 다시 불러오지 못했습니다. 사무실 목록에서 다시 선택해 주세요."
          : "";
}

function snapshot(): Promise<WorldDto> {
  snapshots += 1;
  const fail = failSnapshot;
  failSnapshot = false;
  const result = structuredClone(backend);
  status();
  return new Promise((resolve, reject) => {
    setTimeout(() => (fail ? reject(new Error("검증용 복구 실패")) : resolve(result)), 600);
  });
}

function request(id: string, requestId: number, delay = 80, fail = false): void {
  requests += 1;
  const response = new Promise<SelectResponseDto>((resolve, reject) => {
    setTimeout(() => {
      if (fail) {
        reject(new Error("검증용 선택 실패"));
        return;
      }
      const selected = offices.find((o) => o.officeId === id)!;
      if (requestId >= backend.selectRequest) {
        backend = {
          ...backend,
          listSeq: backend.listSeq + 1,
          selectedId: id,
          selectRequest: requestId,
          selected,
        };
        store.applyList(backend);
        store.applyOfficeState(selected);
      }
      resolve({ ...selected, requestId });
    }, delay);
  });
  void store.requestSelect(requestId, response, snapshot);
  status();
}

function select(id: string, delay = 80, fail = false): void {
  request(id, store.selectOffice(id), delay, fail);
}

store.applyWorld(backend);
store.subscribe(status);
let canvas: OfficeHandle | undefined;
const panel = mountPanel(get("panel"), store);
mountSidebar(get("side"), store, request);
const header = mountHeader(get("app"), get("header"), store, {
  bandSlot: () => get("side").querySelector(".ow-side-band"),
  togglePanel: () => panel.toggleDrawer(),
  setRoomLabels: (on) => canvas?.setRoomLabels(on),
});
mountOffice(get("stage"), store, { onScale: (scale) => header.setScale(scale) }).then(
  (handle) => {
    canvas = handle;
    get("app").dataset.ready = "true";
  },
  (error) => {
    get("status").textContent = `캔버스 로드 실패: ${String(error)}`;
  },
);
get("rapid").addEventListener("click", () => {
  select("A", 700);
  select("B", 80);
});
get("latest-failure").addEventListener("click", () => {
  select("B", 700);
  select("C", 80, true);
});
get("recovery-failure").addEventListener("click", () => {
  failSnapshot = true;
  select("C", 80, true);
});
get("hidden-switch").addEventListener("click", () => {
  setTimeout(() => select(store.state.selectedId === "B" ? "A" : "B"), 1000);
});
status();
