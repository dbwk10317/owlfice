import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import "../design/handoff/tokens.css";
import "./styles/app.css"; // 규모 선택·로딩 화면에도 body 여백 0 이 필요해 가장 먼저 부른다
import { firstOffice } from "./first-office.ts";
import { mountHeader } from "./header.ts";
import { mountOffice, type OfficeHandle } from "./office/renderer.ts";
import { emptyState, mountSidebar } from "./sidebar-left.ts";
import { mountPanel } from "./sidebar-right.ts";
import { showLoading, type LoadingSheet } from "./loading.ts";
import { showScale } from "./scale.ts";
import phrases from "../design/handoff/phrases.ko.json";
import { createStore } from "./store.ts";
import type {
  OfficeDto,
  OfficeListDto,
  SelectResponseDto,
  SettingsDto,
  WorldDto,
} from "./types/ipc.ts";

const store = createStore<OfficeListDto, OfficeDto>();

// 사이드바 클릭과 첫 사무실 자동 선택이 같은 경로로 부른다
function requestSelect(officeId: string, requestId: number): void {
  invoke<SelectResponseDto>("select_office", { officeId, requestId })
    .then(store.applySelectResponse)
    .catch((e) => console.error("select_office", e));
}

// ponytail: 첫 사무실을 여기서 고른다, 로딩 화면(T16)이 "첫 사무실 선택"을 맡으면 옮긴다
store.subscribe(() => {
  const first = firstOffice(store.state.list?.offices ?? []);
  if (store.state.selectedId !== null || !first) return;
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
function mountShell(): void {
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
  const panelHost = el("div", "ow-panel-host");
  main.append(headerEl, stage);
  app.append(side, main, panelHost);
  document.body.append(app);

  let office: OfficeHandle | null = null;
  let header: ReturnType<typeof mountHeader> | null = null;
  const panel = mountPanel(panelHost, store, {
    onDrawerChange: (open) => header?.setDrawerOpen(open),
  });
  mountSidebar(side, store, requestSelect); // 머리말이 띠를 찾으니 먼저
  header = mountHeader(app, headerEl, store, {
    bandSlot: () => side.querySelector<HTMLElement>(".ow-side-band"),
    togglePanel: () => panel.toggleDrawer(),
    setRoomLabels: (on) => office?.setRoomLabels(on),
  });
  mountOffice(stage, store, { onScale: (n) => header?.setScale(n) }).then(
    (o) => {
      office = o;
      o.setRoomLabels(header?.roomLabels() ?? true);
    },
    (e) => {
      console.error("사무실 캔버스", e);
      stage.replaceChildren(
        emptyState(document, phrases.canvas.failTitle, phrases.canvas.failBody),
      );
    },
  );
}

async function start(): Promise<void> {
  // 규모를 아직 안 골랐으면(첫 실행, 손상된 설정) 고를 때까지 진행하지 않는다. 그다음 로딩 화면 (PLAN 3.8)
  const settings = await invoke<SettingsDto>("get_settings");
  const root = document.body.appendChild(document.createElement("div"));
  const sheet = await loadSheet();
  if (!settings.scale) await showScale(root, invoke, sheet);
  await showLoading(root, { invoke, listen }, sheet);
  root.remove();
  mountShell();
  // 구독을 먼저 걸어야 스냅샷과 첫 이벤트 사이의 변경을 놓치지 않는다 (PLAN 4.3 시작 순서)
  await Promise.all([
    listen<OfficeListDto>("office-list", (e) => store.applyList(e.payload)),
    listen<OfficeDto>("office-state", (e) => store.applyOfficeState(e.payload)),
  ]);
  store.applyWorld(await invoke<WorldDto>("get_world_snapshot"));
}

start().catch((e) => console.error("시작 실패", e));
