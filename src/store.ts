// 백엔드가 보내는 전체 스냅샷을 받아 seq 규칙으로 걸러 단일 상태에 둔다 (docs/CONTRACTS.md C4·C9).
// DTO 전체 모양은 ts-rs 가 만든다. 여기서는 규칙에 필요한 필드만 안다.
export interface ListMsg {
  listSeq: number;
  // 백엔드가 스스로 바꾼 선택(팀 퇴장, 규모 변경)과 그 선택을 만든 마지막 요청 번호
  selectedId: string | null;
  selectRequest: number;
}
export interface StateMsg {
  stateSeq: number;
  officeId: string;
}
export interface WorldMsg<S extends StateMsg> extends ListMsg {
  selected: S | null;
}

interface StoreState<L, S> {
  list: L | null;
  selectedId: string | null;
  office: S | null;
  selectionError: "recovering" | "restored" | "failed" | null;
}

export function createStore<L extends ListMsg, S extends StateMsg>() {
  const state: StoreState<L, S> = {
    list: null,
    selectedId: null,
    office: null,
    selectionError: null,
  };
  // 사무실마다 따로 센다. 선택을 옮겨도 지우지 않는다: A→B→A 에서 옛 A 응답이 화면을 되돌리지 못하게
  const lastStateSeq = new Map<string, number>();
  const subscribers = new Set<() => void>();
  // 요청 번호는 단조 증가라 백엔드가 늦게 온 옛 요청을 가려낼 수 있다
  let lastRequestId = 0;
  // 실패한 요청은 백엔드 번호를 올리지 않는다. 번호 발급과 목록 선택 수락의 하한을 나눈다.
  let acceptedRequest = 0;
  const pendingSelects = new Set<Promise<S & { requestId: number }>>();
  let recovery: Map<string, S> | null = null;
  // 스냅샷이 오기 전의 이벤트. null 이면 이미 시작된 것
  let buffer: { list: L[]; state: S[] } | null = { list: [], state: [] };

  const notify = () => subscribers.forEach((fn) => fn());

  function applyList(msg: L): void {
    if (buffer) return void buffer.list.push(msg);
    if (state.list && msg.listSeq < state.list.listSeq) return;
    if (state.selectionError === "restored" && msg.listSeq > (state.list?.listSeq ?? -1))
      state.selectionError = null;
    state.list = msg;
    // 더 새 요청을 보낸 뒤라면 그 응답 전의 백엔드 선택이 사용자 선택을 덮지 않게
    if (msg.selectRequest >= acceptedRequest && msg.selectedId !== state.selectedId) {
      state.selectedId = msg.selectedId;
      state.office = null;
    }
    notify();
  }

  function applyOfficeState(msg: S): void {
    if (buffer) return void buffer.state.push(msg);
    // 복구 대상은 스냅샷이 와야 안다. 기다리는 동안 다른 사무실의 최신 상태도 한 건씩 보관한다.
    if (recovery && msg.stateSeq >= (recovery.get(msg.officeId)?.stateSeq ?? -1))
      recovery.set(msg.officeId, msg);
    if (msg.officeId !== state.selectedId) return;
    if (msg.stateSeq < (lastStateSeq.get(msg.officeId) ?? -1)) return;
    if (
      state.selectionError === "restored" &&
      msg.stateSeq > (lastStateSeq.get(msg.officeId) ?? -1)
    )
      state.selectionError = null;
    lastStateSeq.set(msg.officeId, msg.stateSeq);
    state.office = msg;
    notify();
  }

  /** 시작: 구독을 먼저 걸고 스냅샷을 부른 뒤 이 함수로 넘긴다. 시작 때 한 번만 부른다. */
  function applyWorld(world: L & WorldMsg<S>): void {
    const pending = buffer;
    buffer = null;
    state.list = world;
    state.selectedId = world.selected?.officeId ?? null;
    state.office = world.selected;
    // WebView 만 다시 불러오면 백엔드는 옛 번호를 기억한다. 1부터 다시 세면 모든 선택이 늦은 요청으로 버려진다
    lastRequestId = Math.max(lastRequestId, world.selectRequest);
    acceptedRequest = lastRequestId;
    if (world.selected) lastStateSeq.set(world.selected.officeId, world.selected.stateSeq);
    // 스냅샷 seq 이하는 스냅샷이 이미 담고 있다
    for (const m of pending?.list ?? []) if (m.listSeq > world.listSeq) applyList(m);
    for (const m of pending?.state ?? [])
      if (m.stateSeq > (lastStateSeq.get(m.officeId) ?? -1)) applyOfficeState(m);
    if (world.selected && state.selectedId === world.selected.officeId && !state.office)
      state.office = world.selected;
    notify();
  }

  /** 선택을 옮기고 requestId 를 돌려준다. 호출부가 이 id 로 select_office 를 부른다. */
  function selectOffice(officeId: string): number {
    state.selectedId = officeId;
    state.office = null;
    state.selectionError = null;
    recovery = null;
    lastRequestId += 1;
    acceptedRequest = lastRequestId;
    notify();
    return lastRequestId;
  }

  /** select_office 응답. 마지막으로 보낸 requestId 의 것만, 이후는 office-state 규칙. */
  function applySelectResponse(res: S & { requestId: number }): void {
    if (res.requestId !== lastRequestId) return;
    applyOfficeState(res);
  }

  async function requestSelect(
    requestId: number,
    response: Promise<S & { requestId: number }>,
    getWorld: () => Promise<L & WorldMsg<S>>,
  ): Promise<void> {
    pendingSelects.add(response);
    try {
      applySelectResponse(await response);
    } catch (error) {
      console.error("select_office", error);
      if (requestId !== lastRequestId) return;
      recovery = new Map(state.office ? [[state.office.officeId, state.office]] : []);
      state.selectionError = "recovering";
      notify();
      // C가 실패해도 아직 처리 중인 B가 실제 선택이 될 수 있어 발행된 요청부터 끝낸다.
      await Promise.allSettled(pendingSelects);
      if (requestId !== lastRequestId) return;
      try {
        const world = await getWorld();
        if (requestId !== lastRequestId) return;
        if (!state.list || world.listSeq >= state.list.listSeq) state.list = world;
        const latest = state.list;
        state.selectedId = latest.selectedId;
        acceptedRequest = latest.selectRequest;
        lastRequestId = Math.max(lastRequestId, latest.selectRequest);
        const snapshot = world.selected?.officeId === state.selectedId ? world.selected : null;
        const event = state.selectedId === null ? null : recovery?.get(state.selectedId);
        const office =
          event && (!snapshot || event.stateSeq > snapshot.stateSeq) ? event : snapshot;
        state.office = null;
        if (office && office.stateSeq >= (lastStateSeq.get(office.officeId) ?? -1)) {
          lastStateSeq.set(office.officeId, office.stateSeq);
          state.office = office;
        }
        recovery = null;
        state.selectionError = "restored";
        notify();
      } catch (error) {
        console.error("get_world_snapshot", error);
        if (requestId !== lastRequestId) return;
        recovery = null;
        state.selectionError = "failed";
        notify();
      }
    } finally {
      pendingSelects.delete(response);
    }
  }

  function subscribe(fn: () => void): () => void {
    subscribers.add(fn);
    return () => void subscribers.delete(fn);
  }

  return {
    state,
    applyList,
    applyOfficeState,
    applyWorld,
    selectOffice,
    applySelectResponse,
    requestSelect,
    subscribe,
  };
}
