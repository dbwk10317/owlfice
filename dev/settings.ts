// 설정과 IPC는 메모리 모형이다. 실제 설정·세션·클립보드를 건드리지 않는다.
import "../design/handoff/tokens.css";
import "../src/styles/app.css";
import { setAvatarAnimal } from "../src/avatar.ts";
import { mountHeader } from "../src/header.ts";
import { mountOffice, type OfficeHandle } from "../src/office/renderer.ts";
import { openSettings, saveSettings } from "../src/popup-settings.ts";
import type { Invoke } from "../src/scale.ts";
import { mountSidebar } from "../src/sidebar-left.ts";
import { mountPanel } from "../src/sidebar-right.ts";
import { createStore, createUpdateStore } from "../src/store.ts";
import { act, emp, office, team } from "../src/test-office.ts";
import type { OfficeDto, OfficeListDto, SettingsDto } from "../src/types/ipc.ts";

const get = (id: string) => document.getElementById(id)!;
let settings: SettingsDto = {
  scale: "Large",
  companyName: null,
  bossName: null,
  manualPaths: [],
  sound: false,
  nightMode: "auto",
  osNotifications: true,
  rawDiagnostics: false,
  animal: null,
};
// 합성 진행은 버튼으로 한 단계씩 보내 검수 중에도 표시를 유지한다.
let advanceUpdate: (() => void) | undefined;
const updates = createUpdateStore({
  getVersion: async () => "0.2.0",
  check: async () => ({
    version: "0.2.1",
    downloadAndInstall: (progress) =>
      new Promise<void>((resolve) => {
        progress?.({ event: "Started", data: { contentLength: 1024 } });
        advanceUpdate = () => {
          progress?.({ event: "Progress", data: { chunkLength: 512 } });
          advanceUpdate = () => {
            progress?.({ event: "Progress", data: { chunkLength: 512 } });
            progress?.({ event: "Finished" });
            advanceUpdate = () => {
              advanceUpdate = undefined;
              resolve();
            };
          };
        };
      }),
    close: async () => {},
  }),
});
window.addEventListener("beforeunload", () => updates.destroy(), { once: true });
let failSave = false;
let failCopy = false;
let failGet = false;
let seq = 0;
const store = createStore<OfficeListDto, OfficeDto>();
const sampleTeam = team(
  "sample-team",
  [
    emp("boss", {
      rank: "Bujang",
      name: "김부장",
      activity: act({ room: "TeamLead", pose: "Sit" }),
    }),
  ],
  { deskSlot: 0 },
);

function scene(visit = false): OfficeDto {
  const result = office(
    settings.scale === "Small" ? "vendor/claude" : "sample",
    ++seq,
    [
      {
        ...sampleTeam,
        props: {
          ...sampleTeam.props,
          bossVisit: visit ? "합성 메시지: 배포 상황을 알려 주세요." : null,
        },
      },
    ],
    visit
      ? [
          {
            kind: "BossVisit",
            teamId: "sample-team",
            text: "합성 메시지: 배포 상황을 알려 주세요.",
          },
        ]
      : [],
  );
  return result;
}
function list(selectedId: string | null): OfficeListDto {
  return {
    listSeq: ++seq,
    selectedId,
    selectRequest: 0,
    rebuilt: false,
    scale: settings.scale ?? "Large",
    offices: [
      {
        officeId: settings.scale === "Small" ? "vendor/claude" : "sample",
        building: settings.scale === "Small" ? null : "A",
        floor: 1,
        room: 101,
        label: settings.scale === "Small" ? "1층 Claude 사무실" : "A동 101호",
        phase: "Working",
        dot: "Working",
        asking: false,
        border: "Plain",
        delayed: false,
        teams: [
          {
            teamId: "sample-team",
            bossName: "김부장",
            title: "설정 확인",
            folder: "sample",
            others: 0,
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
      },
    ],
  };
}
const first = scene();
store.applyWorld({ ...list(first.officeId), selected: first });
const invoke: Invoke = async <T>(cmd: string, args?: Record<string, unknown>): Promise<T> => {
  let result: unknown;
  if (cmd === "get_settings") {
    if (failGet) {
      failGet = false;
      throw new Error("검증용 설정 읽기 실패");
    }
    result = structuredClone(settings);
  } else if (cmd === "set_settings") {
    const patch = args?.dto as Partial<SettingsDto>;
    settings = { ...settings, ...patch };
    if (settings.companyName === "") settings.companyName = null;
    if (settings.bossName === "") settings.bossName = null;
    if (patch.scale) {
      const next = scene();
      store.applyList({ ...list(next.officeId), rebuilt: true });
      store.applyOfficeState({ ...next, rebuilt: true });
    }
    if (failSave) {
      failSave = false;
      throw new Error("검증용 디스크 저장 실패 (메모리에는 반영됨)");
    }
  } else if (cmd === "get_diagnostics") result = "합성 진단: <home>\\.claude\\projects";
  else if (cmd === "get_log_folder") result = "C:\\samples\\Owlfice\\logs";
  else get("status").textContent = `호출 확인: ${cmd}`;
  return result as T;
};
const writeClipboard = async (text: string) => {
  if (failCopy) {
    failCopy = false;
    throw new Error("검증용 클립보드 실패");
  }
  get("status").textContent = `복사 확인: ${text}`;
};
const open = (opener: HTMLElement) =>
  openSettings(
    {
      invoke,
      onSettings: applySettings,
      writeClipboard,
      updates,
      onError(message) {
        get("status").textContent = message;
      },
    },
    opener,
  );
let canvas: OfficeHandle | undefined;
const panel = mountPanel(get("panel"), store);
const sidebar = mountSidebar(
  get("side"),
  store,
  (_, requestId) => {
    store.applySelectResponse({ ...scene(), requestId });
  },
  { onSettings: open, onRescan: () => invoke<void>("rescan") },
);
const header = mountHeader(get("app"), get("header"), store, {
  bandSlot: () => get("side").querySelector(".ow-side-band"),
  togglePanel: () => panel.toggleDrawer(),
  setRoomLabels: (on) => canvas?.setRoomLabels(on),
  onSound: (sound) => saveSettings({ invoke, onSettings: applySettings }, { sound }),
  onNightMode: (nightMode) => saveSettings({ invoke, onSettings: applySettings }, { nightMode }),
});
function applySettings(next: SettingsDto): void {
  sidebar.setSettings(next.companyName, next.rawDiagnostics ?? false);
  header.setSettings(next.companyName, next.sound ?? false, next.nightMode ?? "auto");
  canvas?.setBossName(next.bossName);
  setAvatarAnimal(next.animal ?? "owls");
  canvas?.setAnimal(next.animal ?? "owls");
  get("status").textContent =
    `회사: ${next.companyName ?? "Owlfice"} · 사장님: ${next.bossName ?? "사장님"} · 소리: ${next.sound} · OS 알림: ${next.osNotifications} · 원문 진단: ${next.rawDiagnostics}`;
}
mountOffice(get("stage"), store, { onScale: (n) => header.setScale(n) }).then(
  (handle) => {
    canvas = handle;
    canvas.setBossName(settings.bossName);
    canvas.setAnimal(settings.animal ?? "owls");
    get("app").dataset.ready = "true";
  },
  (error) => {
    get("status").textContent = `캔버스 로드 실패: ${String(error)}`;
  },
);
get("update-progress").addEventListener("click", () => advanceUpdate?.());
get("open").addEventListener("click", () => open(get("open")));
get("fail-save").addEventListener("click", () => {
  failSave = true;
  get("status").textContent = "다음 저장은 실패합니다. 설정을 열어 한 항목을 바꿔 주세요.";
});
get("fail-copy").addEventListener("click", () => {
  failCopy = true;
  get("status").textContent = "다음 진단 복사는 실패합니다.";
});
get("fail-get").addEventListener("click", () => {
  failGet = true;
  get("status").textContent = "다음 설정 읽기는 실패합니다.";
});
get("hq").addEventListener("click", () => store.applyList({ ...list(null), selectRequest: 999 }));
get("visit").addEventListener("click", () => {
  const next = scene(true);
  store.applyList({ ...list(next.officeId), selectRequest: 999 });
  store.applyOfficeState(next);
});
applySettings(settings);
