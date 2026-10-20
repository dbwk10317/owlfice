import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import "../design/handoff/tokens.css";
import { showScale, type Sheet } from "./scale.ts";
import { createStore } from "./store.ts";
import type {
  OfficeDto,
  OfficeListDto,
  SelectResponseDto,
  SettingsDto,
  WorldDto,
} from "./types/ipc.ts";

const store = createStore<OfficeListDto, OfficeDto>();

// ponytail: 사이드바·현황판(T17·T21) 전까지 상태를 콘솔에만 찍는다
store.subscribe(() => console.log("store", store.state));

// ponytail: 첫 사무실을 여기서 고른다, 로딩 화면(T16)이 "첫 사무실 선택"을 맡으면 옮긴다
store.subscribe(() => {
  const first = store.state.list?.offices[0];
  if (store.state.selectedId !== null || !first) return;
  const requestId = store.selectOffice(first.officeId);
  invoke<SelectResponseDto>("select_office", { officeId: first.officeId, requestId })
    .then(store.applySelectResponse)
    .catch((e) => console.error("select_office", e));
});

async function loadSheet(): Promise<Sheet | undefined> {
  try {
    const ui = await (await fetch("/ui.json")).json();
    return { frames: ui.frames, size: ui.meta.size };
  } catch (e) {
    console.error("ui.json", e);
    return undefined;
  }
}

async function start(): Promise<void> {
  // 규모를 아직 안 골랐으면(첫 실행, 손상된 설정) 고를 때까지 진행하지 않는다
  const settings = await invoke<SettingsDto>("get_settings");
  if (!settings.scale) {
    const root = document.body.appendChild(document.createElement("div"));
    await showScale(root, invoke, await loadSheet());
    root.remove();
  }
  // 구독을 먼저 걸어야 스냅샷과 첫 이벤트 사이의 변경을 놓치지 않는다 (PLAN 4.3 시작 순서)
  await Promise.all([
    listen<OfficeListDto>("office-list", (e) => store.applyList(e.payload)),
    listen<OfficeDto>("office-state", (e) => store.applyOfficeState(e.payload)),
  ]);
  store.applyWorld(await invoke<WorldDto>("get_world_snapshot"));
}

start().catch((e) => console.error("시작 실패", e));
