import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { createStore } from "./store.ts";
import type { OfficeDto, OfficeListDto, SelectResponseDto, WorldDto } from "./types/ipc.ts";

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

async function start(): Promise<void> {
  // 구독을 먼저 걸어야 스냅샷과 첫 이벤트 사이의 변경을 놓치지 않는다 (PLAN 4.3 시작 순서)
  await Promise.all([
    listen<OfficeListDto>("office-list", (e) => store.applyList(e.payload)),
    listen<OfficeDto>("office-state", (e) => store.applyOfficeState(e.payload)),
  ]);
  store.applyWorld(await invoke<WorldDto>("get_world_snapshot"));
}

start().catch((e) => console.error("시작 실패", e));
