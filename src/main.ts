import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { getVersion } from "@tauri-apps/api/app";
import { check } from "@tauri-apps/plugin-updater";
import "../design/handoff/tokens.css";
import "./styles/app.css"; // 규모 선택·로딩 화면에도 body 여백 0 이 필요해 가장 먼저 부른다
import { firstOffice } from "./first-office.ts";
import { mountEmptyOffice } from "./empty-office.ts";
import { applyNightMode, mountHeader } from "./header.ts";
import { mountOffice, type OfficeHandle } from "./office/renderer.ts";
import { OfficeSound } from "./office/sound.ts";
import { emptyState, mountSidebar, popupNames } from "./sidebar-left.ts";
import { mountPanel } from "./sidebar-right.ts";
import { showLoading, type LoadingSheet } from "./loading.ts";
import { openEmployeeDetail } from "./popup-employee.ts";
import { openSessionDetail } from "./popup-session.ts";
import { detailErrorText } from "./popup-modal.ts";
import { openSettings, saveSettings } from "./popup-settings.ts";
import { showScale } from "./scale.ts";
import phrases from "../design/handoff/phrases.ko.json";
import { createStore, createUpdateStore } from "./store.ts";
import type {
  EmployeeDto,
  OfficeDto,
  OfficeListDto,
  SelectResponseDto,
  SettingsDto,
  WorldDto,
} from "./types/ipc.ts";

const store = createStore<OfficeListDto, OfficeDto>();
const updates = createUpdateStore({ getVersion, check });
window.addEventListener("beforeunload", () => updates.destroy(), { once: true });

// 사이드바 클릭과 첫 사무실 자동 선택이 같은 경로로 부른다
function requestSelect(officeId: string, requestId: number): void {
  void store.requestSelect(
    requestId,
    invoke<SelectResponseDto>("select_office", { officeId, requestId }),
    () => invoke<WorldDto>("get_world_snapshot"),
  );
}

// ponytail: 첫 사무실을 여기서 고른다, 로딩 화면(T16)이 "첫 사무실 선택"을 맡으면 옮긴다
store.subscribe(() => {
  const first = firstOffice(store.state.list?.offices ?? []);
  if (store.state.selectedId !== null || store.state.selectionError || !first) return;
  requestSelect(first.officeId, store.selectOffice(first.officeId));
});

async function loadSheet(): Promise<LoadingSheet | undefined> {
  try {
    const ui = await (await fetch("/ui.json")).json();
    return { frames: ui.frames, size: ui.meta.size, building: ui.owlfice?.building };
  } catch (e) {
    console.error("ui.json", e);
    return undefined;
  }
}

/** 사이드바 · 가운데(머리말 + 무대) · 현황판 (HANDOFF 2.1). 창 너비 구간은 header 가 app.dataset.layout 으로 정한다 */
function mountShell(initialSettings: SettingsDto, sheet?: LoadingSheet): void {
  let settings = initialSettings;
  const sound = new OfficeSound();
  sound.setEnabled(settings.sound ?? false);
  document.addEventListener("pointerdown", () => sound.unlock());
  document.addEventListener("keydown", () => sound.unlock());
  const soundGesture = (on: boolean) => {
    sound.setEnabled(on);
    if (on) sound.unlock();
  };
  const companyName = settings.companyName ?? undefined;
  const el = (tag: string, cls = "") => {
    const e = document.createElement(tag);
    e.className = cls;
    return e;
  };
  const app = el("div", "ow-app");
  const side = el("div");
  const main = el("div", "ow-main");
  const headerEl = el("header");
  const stage = el("div", "ow-stage");
  stage.append(el("div", "ow-stage__floor"));
  const selectionError = el("p", "ow-selection-error");
  selectionError.setAttribute("role", "alert");
  selectionError.hidden = true;
  const settingsError = el("p", "ow-selection-error");
  settingsError.setAttribute("role", "alert");
  settingsError.hidden = true;
  const panelHost = el("div", "ow-panel-host");
  main.append(headerEl, selectionError, settingsError, stage);
  app.append(side, main, panelHost);
  document.body.append(app);
  store.subscribe(() => {
    const error = store.state.selectionError;
    selectionError.hidden = error === null;
    selectionError.textContent =
      error === "recovering"
        ? "사무실을 전환하지 못했습니다. 현재 사무실을 다시 불러오는 중입니다."
        : error === "restored"
          ? "사무실을 전환하지 못해 현재 사무실을 다시 불러왔습니다."
          : error === "failed"
            ? "사무실을 다시 불러오지 못했습니다. 사무실 목록에서 다시 선택해 주세요."
            : "";
  });

  let office: OfficeHandle | null = null;
  let pendingTeam: { officeId: string; teamId: string } | null = null;
  let header: ReturnType<typeof mountHeader> | null = null;

  // 팝업은 지금 고른 사무실의 직원만 이름을 안다. 다른 사무실의 세션 상세는 직원 이름 대신 id 를 보인다
  const live = (teamId: string, agentId: string): EmployeeDto | undefined =>
    store.state.office?.teams
      .find((t) => t.teamId === teamId)
      ?.employees.find((e) => e.agentId === agentId);
  const openEmployee = (
    officeId: string,
    teamId: string,
    agentId: string,
    opener?: HTMLElement,
  ): void =>
    openEmployeeDetail(
      {
        invoke,
        live,
        isTeamOff: (id) => store.state.office?.teams.find((t) => t.teamId === id)?.phase === "Off",
      },
      {
        officeId,
        teamId,
        agentId,
        officeName: popupNames(store.state.list, officeId, teamId).officeName,
        building: store.state.office?.teams.find((t) => t.teamId === teamId)?.building,
      },
      opener,
    );

  const panel = mountPanel(panelHost, store, {
    onDrawerChange: (open) => header?.setDrawerOpen(open),
    onEmployee: (teamId, agentId, opener) => {
      const officeId = store.state.selectedId;
      if (officeId) openEmployee(officeId, teamId, agentId, opener);
    },
    onSession: (teamId, opener) => {
      const officeId = store.state.selectedId;
      if (officeId)
        openSessionDetail(
          { invoke, live },
          {
            officeId,
            teamId,
            name: popupNames(store.state.list, officeId, teamId).teamLabel,
          },
          opener,
        );
    },
    onSource: async (teamId) => {
      const officeId = store.state.selectedId;
      if (!officeId) return;
      try {
        await invoke<void>("open_path", { target: { kind: "SourceFolder", officeId, teamId } });
      } catch (error) {
        throw new Error(detailErrorText(error));
      }
    },
  });
  const sidebar = mountSidebar(side, store, requestSelect, {
    companyName,
    rawDiagnostics: settings.rawDiagnostics ?? false,
    onSettings: showSettings,
    onRescan: () => invoke<void>("rescan"),
    onTeam: (officeId, teamId) => {
      panel.showTeam(officeId, teamId);
      if (office) office.highlightTeam(officeId, teamId);
      else pendingTeam = { officeId, teamId };
    },
    onDetail: (officeId, teamId, name, opener) =>
      openSessionDetail({ invoke, live }, { officeId, teamId, name }, opener),
  }); // 머리말이 띠를 찾으니 먼저
  header = mountHeader(app, headerEl, store, {
    companyName,
    sound: settings.sound ?? false,
    nightMode: settings.nightMode ?? "auto",
    onNightMode: (nightMode) => saveSettings({ invoke, onSettings: applySettings }, { nightMode }),
    onSound: (on) => {
      soundGesture(on);
      return saveSettings({ invoke, onSettings: applySettings }, { sound: on });
    },
    bandSlot: () => side.querySelector<HTMLElement>(".ow-side-band"),
    togglePanel: () => panel.toggleDrawer(),
    setRoomLabels: (on) => office?.setRoomLabels(on),
    setScale: (scale) => office?.setScale(scale),
    stage,
    centerCanvas: () => office?.centerCanvas(),
    toggleMaximize: () => getCurrentWindow().toggleMaximize(),
  });
  const emptyOffice = mountEmptyOffice(stage, store, { invoke, onSettings: showSettings, sheet });
  let updateStarted = false;
  const stopUpdateStart = store.subscribe(checkAfterOffice);
  function checkAfterOffice(): void {
    const { list, office: selected } = store.state;
    if (updateStarted || !office || !list || (!selected && list.offices.length > 0)) return;
    updateStarted = true;
    stopUpdateStart();
    // 렌더러의 첫 프레임 다음에 확인해 시작 화면을 네트워크 응답으로 늦추지 않는다.
    requestAnimationFrame(() =>
      requestAnimationFrame(() => {
        void updates.check(true).then((version) => {
          if (version) sidebar.notifyUpdate(version);
        });
      }),
    );
  }
  mountOffice(stage, store, {
    sound,
    bossName: settings.bossName,
    onScale: (n, maxFit) => header?.setScale(n, maxFit),
    onEmployee: (officeId, teamId, agentId) => {
      panel.highlight(teamId, agentId);
      openEmployee(officeId, teamId, agentId);
    },
    onSession: (officeId, teamId) =>
      openSessionDetail(
        { invoke, live },
        { officeId, teamId, name: popupNames(store.state.list, officeId, teamId).teamLabel },
      ),
  }).then(
    (o) => {
      office = o;
      if (pendingTeam) o.highlightTeam(pendingTeam.officeId, pendingTeam.teamId);
      pendingTeam = null;
      o.setRoomLabels(header?.roomLabels() ?? true);
      o.setBossName(settings.bossName);
      checkAfterOffice();
    },
    (e) => {
      console.error("사무실 캔버스", e);
      emptyOffice.dispose();
      stage.replaceChildren(
        emptyState(document, phrases.canvas.failTitle, phrases.canvas.failBody),
      );
    },
  );
  function showSettings(opener?: HTMLElement): void {
    settingsError.hidden = true;
    openSettings(
      {
        invoke,
        updates,
        onSettings: applySettings,
        onSoundGesture: soundGesture,
        onError(message) {
          settingsError.textContent = message;
          settingsError.hidden = false;
        },
      },
      opener,
    );
  }
  function applySettings(next: SettingsDto): void {
    settings = next;
    sound.setEnabled(next.sound ?? false);
    sidebar.setSettings(next.companyName, next.rawDiagnostics ?? false);
    header?.setSettings(next.companyName, next.sound ?? false, next.nightMode ?? "auto");
    office?.setBossName(next.bossName);
  }
}

async function start(): Promise<void> {
  // 규모를 아직 안 골랐으면(첫 실행, 손상된 설정) 고를 때까지 진행하지 않는다. 그다음 로딩 화면 (docs/CONTRACTS.md C4)
  let settings = await invoke<SettingsDto>("get_settings");
  applyNightMode(document, settings.nightMode ?? "auto");
  const root = document.body.appendChild(document.createElement("div"));
  const sheet = await loadSheet();
  if (!settings.scale) {
    await showScale(root, invoke, sheet);
    settings = await invoke<SettingsDto>("get_settings");
  }
  const companyName = settings.companyName ?? undefined;
  await showLoading(root, { invoke, listen }, sheet, companyName);
  root.remove();
  mountShell(settings, sheet);
  // 구독을 먼저 걸어야 스냅샷과 첫 이벤트 사이의 변경을 놓치지 않는다 (docs/CONTRACTS.md C4 시작 순서)
  await Promise.all([
    listen<OfficeListDto>("office-list", (e) => store.applyList(e.payload)),
    listen<OfficeDto>("office-state", (e) => store.applyOfficeState(e.payload)),
  ]);
  store.applyWorld(await invoke<WorldDto>("get_world_snapshot"));
}

function sendVisibility(): void {
  void invoke("set_window_hidden", { hidden: document.hidden }).catch((e) =>
    console.error("창 표시 상태 전송 실패", e),
  );
}
document.addEventListener("visibilitychange", sendVisibility);
sendVisibility();

start().catch((e) => {
  console.error("시작 실패", e);
  const screen = document.createElement("div");
  screen.className = "ow-app ow-start-error";
  const message = emptyState(
    document,
    "앱을 시작하지 못했습니다",
    "설정과 사무실 정보를 불러오지 못했어요. 다시 시도해 주세요.",
  );
  message.setAttribute("role", "alert");
  const retry = document.createElement("button");
  retry.type = "button";
  retry.className = "ow-btn";
  retry.textContent = "다시 시도";
  retry.addEventListener("click", () => location.reload());
  message.append(retry);
  screen.append(message);
  document.body.replaceChildren(screen);
});
