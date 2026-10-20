import { expect, test, vi } from "vite-plus/test";
import { createStore, type ListMsg, type StateMsg } from "./store.ts";

type L = ListMsg & { offices: string[] };
type S = StateMsg & { tag: string };

const list = (listSeq: number, ...offices: string[]): L => ({ listSeq, offices });
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
  s.applySelectResponse({ ...st("B", 3, "b"), requestId: "r1" });
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
  s.applySelectResponse({ ...st("A", 2, "x"), requestId: "r0" });
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
