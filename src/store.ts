// 백엔드가 보내는 전체 스냅샷을 받아 seq 규칙으로 걸러 단일 상태에 둔다 (PLAN 4.3, 4.8).
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
}

export function createStore<L extends ListMsg, S extends StateMsg>() {
  const state: StoreState<L, S> = { list: null, selectedId: null, office: null };
  // 사무실마다 따로 센다. 선택을 옮겨도 지우지 않는다: A→B→A 에서 옛 A 응답이 화면을 되돌리지 못하게
  const lastStateSeq = new Map<string, number>();
  const subscribers = new Set<() => void>();
  // 요청 번호는 단조 증가라 백엔드가 늦게 온 옛 요청을 가려낼 수 있다
  let lastRequestId = 0;
  // 스냅샷이 오기 전의 이벤트. null 이면 이미 시작된 것
  let buffer: { list: L[]; state: S[] } | null = { list: [], state: [] };

  const notify = () => subscribers.forEach((fn) => fn());

  function applyList(msg: L): void {
    if (buffer) return void buffer.list.push(msg);
    if (state.list && msg.listSeq < state.list.listSeq) return;
    state.list = msg;
    // 더 새 요청을 보낸 뒤라면 그 응답 전의 백엔드 선택이 사용자 선택을 덮지 않게
    if (msg.selectRequest >= lastRequestId && msg.selectedId !== state.selectedId) {
      state.selectedId = msg.selectedId;
      state.office = null;
    }
    notify();
  }

  function applyOfficeState(msg: S): void {
    if (buffer) return void buffer.state.push(msg);
    if (msg.officeId !== state.selectedId) return;
    if (msg.stateSeq < (lastStateSeq.get(msg.officeId) ?? -1)) return;
    lastStateSeq.set(msg.officeId, msg.stateSeq);
    state.office = msg;
    notify();
  }

  /** 시작: 구독을 먼저 걸고 스냅샷을 부른 뒤 이 함수로 넘긴다. 시작 때 한 번만 부른다 (다시 맞추기는 아직 없다). */
  function applyWorld(world: L & WorldMsg<S>): void {
    const pending = buffer;
    buffer = null;
    state.list = world;
    state.selectedId = world.selected?.officeId ?? null;
    state.office = world.selected;
    // WebView 만 다시 불러오면 백엔드는 옛 번호를 기억한다. 1부터 다시 세면 모든 선택이 늦은 요청으로 버려진다
    lastRequestId = Math.max(lastRequestId, world.selectRequest);
    if (world.selected) lastStateSeq.set(world.selected.officeId, world.selected.stateSeq);
    // 스냅샷 seq 이하는 스냅샷이 이미 담고 있다
    for (const m of pending?.list ?? []) if (m.listSeq > world.listSeq) applyList(m);
    for (const m of pending?.state ?? [])
      if (m.stateSeq > (lastStateSeq.get(m.officeId) ?? -1)) applyOfficeState(m);
    notify();
  }

  /** 선택을 옮기고 requestId 를 돌려준다. 호출부가 이 id 로 select_office 를 부른다. */
  // ponytail: select_office 가 실패하면 화면이 빈 채로 남는다, 사무실 전환 티켓(T28)에서 되돌리기를 정한다
  function selectOffice(officeId: string): number {
    state.selectedId = officeId;
    state.office = null;
    lastRequestId += 1;
    notify();
    return lastRequestId;
  }

  /** select_office 응답. 마지막으로 보낸 requestId 의 것만, 이후는 office-state 규칙. */
  function applySelectResponse(res: S & { requestId: number }): void {
    if (res.requestId !== lastRequestId) return;
    applyOfficeState(res);
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
    subscribe,
  };
}
