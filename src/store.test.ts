import { afterEach, beforeEach, describe, expect, test, vi } from "vite-plus/test";
import { createStore, type ListMsg, type StateMsg } from "./store.ts";

type L = ListMsg & { offices: string[] };
type S = StateMsg & { tag: string };

// 기본은 스냅샷의 선택(A)을 그대로 두는 목록
const list = (listSeq: number, ...offices: string[]): L => ({
  listSeq,
  offices,
  selectedId: "A",
  selectRequest: 0,
});
const st = (officeId: string, stateSeq: number, tag = ""): S => ({ officeId, stateSeq, tag });

function started(selected: S | null = st("A", 1, "a1")) {
  const s = createStore<L, S>();
  s.applyWorld({ ...list(5, "A", "B"), selected });
  return s;
}

test("list: 작은 seq 는 버리고 같은 seq 는 다시 적용한다", () => {
  const s = started();
  s.applyList(list(4, "old"));
  expect(s.state.list?.offices).toEqual(["A", "B"]);
  s.applyList(list(5, "same"));
  expect(s.state.list?.offices).toEqual(["same"]);
  s.applyList(list(6, "new"));
  expect(s.state.list?.offices).toEqual(["new"]);
});

test("state: 선택과 다른 office 는 버린다", () => {
  const s = started();
  s.applyOfficeState(st("B", 9, "b"));
  expect(s.state.office?.tag).toBe("a1");
});

test("state: 작은 seq 는 버리고 같은 seq 는 적용한다", () => {
  const s = started();
  s.applyOfficeState(st("A", 0, "old"));
  expect(s.state.office?.tag).toBe("a1");
  s.applyOfficeState(st("A", 1, "same"));
  expect(s.state.office?.tag).toBe("same");
});

test("A→B→A, 중간 변경 없음: 같은 seq 응답으로 A 가 다시 그려진다", () => {
  const s = started();
  s.selectOffice("B");
  s.applySelectResponse({ ...st("B", 3, "b"), requestId: 1 });
  const id = s.selectOffice("A");
  expect(s.state.office).toBeNull();
  s.applySelectResponse({ ...st("A", 1, "a1"), requestId: id });
  expect(s.state.office?.tag).toBe("a1");
});

test("B 를 고른 뒤 늦게 온 A 응답과 A 이벤트는 버린다", () => {
  const s = started();
  const idA = s.selectOffice("A");
  const idB = s.selectOffice("B");
  s.applySelectResponse({ ...st("A", 2, "a2"), requestId: idA });
  s.applyOfficeState(st("A", 2, "a2"));
  expect(s.state.office).toBeNull();
  s.applySelectResponse({ ...st("B", 3, "b"), requestId: idB });
  expect(s.state.office?.tag).toBe("b");
});

test("선택과 다른 사무실의 이벤트는 응답 없이도 버린다", () => {
  const s = started();
  s.selectOffice("B");
  s.applyOfficeState(st("A", 9, "a"));
  expect(s.state.office).toBeNull();
});

test("select 응답: 옛 requestId 는 버린다", () => {
  const s = started();
  s.selectOffice("A");
  s.applySelectResponse({ ...st("A", 2, "x"), requestId: 0 });
  expect(s.state.office).toBeNull();
});

test("select 응답: 이미 받은 이벤트보다 과거면 버린다", () => {
  const s = started();
  const id = s.selectOffice("A");
  s.applyOfficeState(st("A", 4, "event"));
  s.applySelectResponse({ ...st("A", 3, "stale"), requestId: id });
  expect(s.state.office?.tag).toBe("event");
});

test("시작 버퍼: 스냅샷 seq 이하는 버리고 초과는 적용한다", () => {
  const s = createStore<L, S>();
  s.applyList(list(5, "eq"));
  s.applyList(list(6, "gt"));
  s.applyOfficeState(st("A", 1, "eq"));
  s.applyOfficeState(st("A", 2, "gt"));
  s.applyOfficeState(st("B", 9, "other"));
  expect(s.state.list).toBeNull();
  s.applyWorld({ ...list(5, "snap"), selected: st("A", 1, "snap") });
  expect(s.state.list?.offices).toEqual(["gt"]);
  expect(s.state.office?.tag).toBe("gt");
});

test("시작 버퍼: A→B→A 뒤 새 A 상태가 없으면 스냅샷을 복원한다", () => {
  const s = createStore<L, S>();
  s.applyList({ ...list(2), selectedId: "B" });
  s.applyList(list(3));
  s.applyOfficeState(st("B", 1, "b"));
  s.applyOfficeState(st("A", 1, "same seq cue"));
  s.applyWorld({ ...list(1), selected: st("A", 1, "snapshot") });
  expect(s.state.selectedId).toBe("A");
  expect(s.state.office?.tag).toBe("snapshot");
});

test("시작 버퍼: A→B→A 뒤 더 새 A 상태는 유지한다", () => {
  const s = createStore<L, S>();
  s.applyList({ ...list(2), selectedId: "B" });
  s.applyList(list(3));
  s.applyOfficeState(st("A", 2, "new"));
  s.applyWorld({ ...list(1), selected: st("A", 1, "snapshot") });
  expect(s.state.office?.tag).toBe("new");
});

test("시작 버퍼: 최종 선택 B이면 A 스냅샷을 복원하지 않는다", () => {
  const s = createStore<L, S>();
  s.applyList({ ...list(2), selectedId: "B" });
  s.applyWorld({ ...list(1), selected: st("A", 1, "snapshot") });
  expect(s.state.selectedId).toBe("B");
  expect(s.state.office).toBeNull();
});

test("구독자는 적용될 때만 불리고 해제하면 안 불린다", () => {
  const s = started();
  const fn = vi.fn();
  const off = s.subscribe(fn);
  s.applyList(list(1, "dropped"));
  expect(fn).not.toHaveBeenCalled();
  s.applyOfficeState(st("B", 9, "other"));
  expect(fn).not.toHaveBeenCalled();
  s.applyList(list(6, "ok"));
  expect(fn).toHaveBeenCalledTimes(1);
  off();
  s.applyList(list(7, "ok2"));
  expect(fn).toHaveBeenCalledTimes(1);
});

test("목록: 백엔드가 바꾼 선택(규모 변경)을 따르고 그 사무실 상태를 받는다", () => {
  const s = started();
  s.applyList({ ...list(6), selectedId: "vendor/claude" });
  expect(s.state.selectedId).toBe("vendor/claude");
  expect(s.state.office).toBeNull();
  s.applyOfficeState(st("vendor/claude", 1, "v"));
  expect(s.state.office?.tag).toBe("v");
});

test("목록: 백엔드가 선택을 풀면(팀 퇴장) 선택이 빈다", () => {
  const s = started();
  s.applyList({ ...list(6), selectedId: null });
  expect(s.state.selectedId).toBeNull();
  expect(s.state.office).toBeNull();
});

test("목록: 더 새 요청을 보낸 뒤의 옛 선택은 사용자 선택을 덮지 않는다", () => {
  const s = started();
  const id = s.selectOffice("B");
  s.applyList({ ...list(6), selectedId: "A", selectRequest: id - 1 });
  expect(s.state.selectedId).toBe("B");
  s.applySelectResponse({ ...st("B", 3, "b"), requestId: id });
  s.applyList({ ...list(7), selectedId: "B", selectRequest: id });
  expect(s.state.office?.tag).toBe("b");
});

test("다시 불러온 WebView 는 스냅샷의 요청 번호부터 이어 센다", () => {
  const s = createStore<L, S>();
  s.applyWorld({ ...list(5, "A"), selectRequest: 7, selected: st("A", 1) });
  expect(s.selectOffice("B")).toBe(8);
});

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((yes, no) => {
    resolve = yes;
    reject = no;
  });
  return { promise, resolve, reject };
}

describe("선택 실패 복구", () => {
  beforeEach(() => vi.spyOn(console, "error").mockImplementation(() => {}));
  afterEach(() => vi.restoreAllMocks());

  test("빠른 A→B 선택의 늦은 성공 응답도 최신 B 이벤트를 되돌리지 않는다", async () => {
    const s = started();
    const a = deferred<S & { requestId: number }>();
    const b = deferred<S & { requestId: number }>();
    const getWorld = vi.fn();
    const idA = s.selectOffice("A");
    const pendingA = s.requestSelect(idA, a.promise, getWorld);
    const idB = s.selectOffice("B");
    const pendingB = s.requestSelect(idB, b.promise, getWorld);
    s.applyOfficeState(st("B", 4, "new B"));
    b.resolve({ ...st("B", 3, "old B"), requestId: idB });
    a.resolve({ ...st("A", 99, "late A"), requestId: idA });
    await Promise.all([pendingA, pendingB]);
    expect(s.state.office?.tag).toBe("new B");
    expect(getWorld).not.toHaveBeenCalled();
  });

  test("옛 복구 스냅샷의 실패도 새 선택에 오류를 남기지 않는다", async () => {
    const s = started();
    const snapshot = deferred<L & { selected: S | null }>();
    const reading = deferred<void>();
    const recovery = s.requestSelect(s.selectOffice("B"), Promise.reject(new Error("실패")), () => {
      reading.resolve();
      return snapshot.promise;
    });
    await reading.promise;
    const id = s.selectOffice("C");
    await s.requestSelect(id, Promise.resolve({ ...st("C", 1, "C"), requestId: id }), vi.fn());
    snapshot.reject(new Error("옛 복구 실패"));
    await recovery;
    expect(s.state.office?.tag).toBe("C");
    expect(s.state.selectionError).toBeNull();
  });

  test("최신 선택 실패는 실제 백엔드 선택을 복원하고 다음 요청 번호는 계속 증가한다", async () => {
    const s = started();
    const id = s.selectOffice("B");
    await s.requestSelect(id, Promise.reject(new Error("실패")), async () => ({
      ...list(6, "A", "B"),
      selected: st("A", 2, "actual"),
    }));
    expect(s.state.selectedId).toBe("A");
    expect(s.state.office?.tag).toBe("actual");
    expect(s.state.selectionError).toBe("restored");
    // 실패한 요청 번호보다 낮아도 복구 뒤 백엔드가 바꾼 선택은 받는다.
    s.applyList({ ...list(7), selectedId: "B" });
    expect(s.state.selectedId).toBe("B");
    expect(s.selectOffice("A")).toBe(id + 1);
    expect(s.state.selectionError).toBeNull();
  });

  test("A→B→C에서 C 실패 뒤 늦게 처리된 B까지 기다려 스냅샷을 한 번 읽는다", async () => {
    const s = started();
    const b = deferred<S & { requestId: number }>();
    const getWorld = vi.fn(async () => ({
      ...list(6),
      selectedId: "B",
      selectRequest: 1,
      selected: st("B", 2, "actual B"),
    }));
    const idB = s.selectOffice("B");
    const pendingB = s.requestSelect(idB, b.promise, getWorld);
    const idC = s.selectOffice("C");
    const pendingC = s.requestSelect(idC, Promise.reject(new Error("C 실패")), getWorld);
    await Promise.resolve();
    expect(s.state.selectionError).toBe("recovering");
    expect(getWorld).not.toHaveBeenCalled();
    b.resolve({ ...st("B", 2), requestId: idB });
    await Promise.all([pendingB, pendingC]);
    expect(getWorld).toHaveBeenCalledTimes(1);
    expect(s.state.selectedId).toBe("B");
    expect(s.state.office?.tag).toBe("actual B");
  });

  test("옛 실패는 현재 선택을 건드리거나 재동기화하지 않는다", async () => {
    const s = started();
    const a = deferred<S & { requestId: number }>();
    const getWorld = vi.fn();
    const idA = s.selectOffice("A");
    const pendingA = s.requestSelect(idA, a.promise, getWorld);
    const idB = s.selectOffice("B");
    await s.requestSelect(idB, Promise.resolve({ ...st("B", 1, "B"), requestId: idB }), getWorld);
    a.reject(new Error("옛 실패"));
    await pendingA;
    expect(getWorld).not.toHaveBeenCalled();
    expect(s.state.office?.tag).toBe("B");
    expect(s.state.selectionError).toBeNull();
  });

  test("복구 스냅샷을 기다리는 동안 새 선택이 오면 옛 스냅샷은 버린다", async () => {
    const s = started();
    const snapshot = deferred<L & { selected: S | null }>();
    const reading = deferred<void>();
    const idB = s.selectOffice("B");
    const recovery = s.requestSelect(idB, Promise.reject(new Error("실패")), () => {
      reading.resolve();
      return snapshot.promise;
    });
    await reading.promise;
    const idC = s.selectOffice("C");
    await s.requestSelect(idC, Promise.resolve({ ...st("C", 1, "C"), requestId: idC }), vi.fn());
    snapshot.resolve({ ...list(99), selected: st("A", 99, "old") });
    await recovery;
    expect(s.state.selectedId).toBe("C");
    expect(s.state.office?.tag).toBe("C");
    expect(s.state.list?.listSeq).toBe(5);
    expect(s.state.selectionError).toBeNull();
  });

  test("기존 요청을 기다리는 중 새 선택이 오면 복구 스냅샷 자체를 요청하지 않는다", async () => {
    const s = started();
    const b = deferred<S & { requestId: number }>();
    const getWorld = vi.fn();
    const idB = s.selectOffice("B");
    const pendingB = s.requestSelect(idB, b.promise, getWorld);
    const pendingC = s.requestSelect(
      s.selectOffice("C"),
      Promise.reject(new Error("실패")),
      getWorld,
    );
    await Promise.resolve();
    const idA = s.selectOffice("A");
    await s.requestSelect(
      idA,
      Promise.resolve({ ...st("A", 2, "new A"), requestId: idA }),
      getWorld,
    );
    b.resolve({ ...st("B", 2), requestId: idB });
    await Promise.all([pendingB, pendingC]);
    expect(getWorld).not.toHaveBeenCalled();
    expect(s.state.office?.tag).toBe("new A");
  });

  test("복구 대상의 더 새 이벤트와 목록을 오래된 스냅샷으로 되돌리지 않는다", async () => {
    const s = started();
    const id = s.selectOffice("C");
    await s.requestSelect(id, Promise.reject(new Error("실패")), async () => {
      s.applyList({ ...list(9, "new list"), selectedId: "B" });
      s.applyOfficeState(st("B", 8, "new B"));
      s.applyOfficeState(st("B", 7, "older event"));
      return { ...list(8, "old list"), selectedId: "B", selected: st("B", 6, "old snapshot") };
    });
    expect(s.state.list?.offices).toEqual(["new list"]);
    expect(s.state.selectedId).toBe("B");
    expect(s.state.office?.tag).toBe("new B");
    s.applyOfficeState(st("B", 7, "stale"));
    expect(s.state.office?.tag).toBe("new B");
  });

  test("스냅샷보다 새 목록이 다른 선택을 가리키면 옛 사무실을 넣지 않고 상태를 기다린다", async () => {
    const s = started();
    await s.requestSelect(s.selectOffice("C"), Promise.reject(new Error("실패")), async () => {
      s.applyList({ ...list(9), selectedId: "B" });
      return { ...list(8), selected: st("A", 6, "old office") };
    });
    expect(s.state.selectedId).toBe("B");
    expect(s.state.office).toBeNull();
    s.applyOfficeState(st("B", 7, "new office"));
    expect(s.state.office?.tag).toBe("new office");
  });

  test("이미 적용한 stateSeq보다 오래된 복구 상태는 버린다", async () => {
    const s = started(st("A", 10, "new"));
    await s.requestSelect(s.selectOffice("B"), Promise.reject(new Error("실패")), async () => ({
      ...list(6),
      selected: st("A", 9, "stale"),
    }));
    expect(s.state.office).toBeNull();
    s.applyOfficeState(st("A", 10, "same"));
    expect(s.state.office?.tag).toBe("same");
  });

  test("복구도 실패하면 오류를 남기고 재시도하지 않으며 다음 선택을 허용한다", async () => {
    const s = started();
    const getWorld = vi.fn(async () => {
      throw new Error("스냅샷 실패");
    });
    await s.requestSelect(s.selectOffice("B"), Promise.reject(new Error("실패")), getWorld);
    expect(getWorld).toHaveBeenCalledTimes(1);
    expect(s.state.selectionError).toBe("failed");
    const id = s.selectOffice("B");
    await s.requestSelect(id, Promise.resolve({ ...st("B", 3, "retry"), requestId: id }), getWorld);
    expect(s.state.office?.tag).toBe("retry");
    expect(s.state.selectionError).toBeNull();
  });
});
