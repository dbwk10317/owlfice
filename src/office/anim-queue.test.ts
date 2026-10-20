import { expect, test } from "vite-plus/test";
import rawFloorplan from "../../design/sprites/floorplan.json";
import {
  CLAP_MS,
  ENTER_MS,
  HIDDEN,
  MAX_STEPS,
  Track,
  WALK_MAX_MS,
  WALK_MIN_MS,
  canWander,
  facing,
  idlePeriod,
  walkMs,
  walkPath,
  type Pt,
  type Target,
} from "./anim-queue.ts";
import { parseFloorplan } from "./floorplan.ts";

const fp = parseFloorplan(rawFloorplan);
const NOW = 1_000_000;

const pt = (name: string, i = 0): Pt => {
  const [gx, gy, z = 0] = fp.stands[name][i];
  return { gx, gy, z };
};
const entry = (room: string): Pt => ({
  gx: fp.rooms[room].entry[0],
  gy: fp.rooms[room].entry[1],
  z: 0,
});
const SEAT: Pt = { gx: 2.75, gy: 6.25, z: 0 }; // dev_0
const SIT = { pose: "sit", dir: "f" as const };

function target(at: Pt | null, o: Partial<Target> = {}): Target {
  return { at, look: SIT, depth: at ? at.gx + at.gy : 0, wander: false, pace: false, ...o };
}

function track(o: { seed?: number; reduced?: boolean } = {}) {
  return new Track({ fp, seed: o.seed ?? 0, reduced: () => o.reduced ?? false });
}

// ---------- 경로·시간·방향 ----------

test("같은 방은 직선 하나, 다른 방은 방 entry 와 개발실 통로를 지난다", () => {
  const a = pt("test", 0);
  const b = pt("test", 1);
  expect(walkPath(fp, a, b)).toEqual([a, b]);
  // 개발실 좌석 → 테스트실: 개발실 entry(통로) + 테스트실 entry, 모서리 두 점
  const to = pt("test");
  expect(walkPath(fp, SEAT, to)).toEqual([SEAT, entry("dev"), entry("test"), to]);
  // 테스트실 → 서고: 나가는 방 entry → 통로 → 들어가는 방 entry
  const lib = pt("library");
  expect(walkPath(fp, to, lib)).toEqual([to, entry("test"), entry("dev"), entry("library"), lib]);
});

test("걷는 시간은 화면 거리로 정하고 0.5–1.5초로 자른다", () => {
  // 한 타일 남짓 → 0.5초
  expect(walkMs([SEAT, { ...SEAT, gx: SEAT.gx + 1 }])).toBe(WALK_MIN_MS);
  // 방 사이 → 1.5초
  expect(walkMs(walkPath(fp, SEAT, pt("library")))).toBe(WALK_MAX_MS);
  // boss_pace 두 자리(36px)는 그 사이
  const pace = walkMs([pt("boss_pace", 0), pt("boss_pace", 1)]);
  expect(pace).toBeGreaterThan(WALK_MIN_MS);
  expect(pace).toBeLessThan(WALK_MAX_MS);
});

test("아래로 가면 f, 위로 가면 b, 화면 왼쪽으로 가면 뒤집는다", () => {
  const o: Pt = { gx: 5, gy: 5, z: 0 };
  // gx+ 는 화면 오른쪽 아래
  expect(facing(o, { ...o, gx: 6 })).toEqual({ dir: "f", flip: false });
  // gy+ 는 왼쪽 아래
  expect(facing(o, { ...o, gy: 6 })).toEqual({ dir: "f", flip: true });
  // gx− 는 왼쪽 위
  expect(facing(o, { ...o, gx: 4 })).toEqual({ dir: "b", flip: true });
  // gy− 는 오른쪽 위
  expect(facing(o, { ...o, gy: 4 })).toEqual({ dir: "b", flip: false });
});

// ---------- 큐 ----------

test("자리가 바뀌면 walk 로 걷고, 도착하면 그 자리 자세로 반전 없이 선다", () => {
  const t = track();
  t.reset(target(SEAT), NOW);
  const to = pt("library");
  const tgt = target(to, { look: { pose: "carry", dir: "f", held: "folder" } });
  t.retarget(tgt, NOW);
  // 첫 구간은 좌석 → 개발실 통로(오른쪽 아래)
  const s0 = t.update(NOW + 1);
  expect(s0.look).toEqual({ pose: "walk", dir: "f" });
  expect(s0.flip).toBe(false);
  // 통로 → 서고 entry 는 왼쪽 위 (경로 299px 중 131~260px)
  const s1 = t.update(NOW + WALK_MAX_MS * 0.65);
  expect(s1.look).toEqual({ pose: "walk", dir: "b" });
  expect(s1.flip).toBe(true);
  // entry → 서가 앞은 오른쪽 위
  const s2 = t.update(NOW + WALK_MAX_MS - 1);
  expect(s2.look).toEqual({ pose: "walk", dir: "b" });
  expect(s2.flip).toBe(false);
  const end = t.update(NOW + WALK_MAX_MS);
  expect(end.at).toEqual(to);
  expect(end.look).toBe(tgt.look);
  expect(end.flip).toBe(false);
  expect(end.depth).toBe(tgt.depth);
});

test("걷는 중에 온 다음 자리는 앞 걷기가 끝난 곳에서 이어 걷는다", () => {
  const t = track();
  t.reset(target(SEAT), NOW);
  t.retarget(target(pt("test")), NOW);
  t.retarget(target(pt("build")), NOW + 100);
  expect(t.length).toBe(2);
  // 첫 걷기가 끝난 순간 테스트실에 있다
  expect(t.update(NOW + WALK_MAX_MS).at).toEqual(pt("test"));
  expect(t.update(NOW + 2 * WALK_MAX_MS).at).toEqual(pt("build"));
});

test(`큐가 ${MAX_STEPS}개 넘게 쌓이면 중간을 건너뛰고 마지막 자리로 점프`, () => {
  const t = track();
  t.reset(target(SEAT), NOW);
  const spots = [pt("test"), pt("build"), pt("library"), pt("meeting"), pt("shipping")];
  for (const s of spots) t.retarget(target(s), NOW);
  expect(t.length).toBe(MAX_STEPS);
  const last = target(pt("review_sofa"));
  t.retarget(last, NOW);
  expect(t.length).toBe(0);
  expect(t.update(NOW).at).toEqual(last.at);
});

test("reset 은 큐를 비우고 바로 제자리 (조용한 재구성·사무실 전환)", () => {
  const t = track();
  t.reset(target(SEAT), NOW);
  t.retarget(target(pt("test")), NOW);
  const to = target(pt("library"));
  t.reset(to, NOW + 10);
  expect(t.length).toBe(0);
  expect(t.update(NOW + 10).at).toEqual(to.at);
});

test("출근은 문에서 좌석까지 4초 걷는다", () => {
  const t = track();
  t.retarget(target(SEAT), NOW, true);
  expect(t.update(NOW).at).toEqual(pt("door"));
  const mid = t.update(NOW + ENTER_MS - 1);
  expect(mid.look.pose).toBe("walk");
  expect(t.update(NOW + ENTER_MS).at).toEqual(SEAT);
});

test("숨기면 출구까지 걸어가 사라지고, 다시 보이면 문에서 들어온다", () => {
  const t = track();
  t.reset(target(SEAT), NOW);
  t.retarget(HIDDEN, NOW);
  expect(t.update(NOW + 1).at).not.toBeNull();
  const ms = walkMs(walkPath(fp, SEAT, pt("exit")));
  expect(t.update(NOW + ms - 1).at).not.toBeNull();
  expect(t.update(NOW + ms).at).toBeNull();
  // 웹 복귀
  t.retarget(target(SEAT), NOW + ms);
  expect(t.update(NOW + ms).at).toEqual(pt("door"));
});

test("clap 은 앞 걷기가 끝난 뒤 1초, 그다음 자리 자세", () => {
  const t = track();
  t.reset(target(SEAT), NOW);
  const center = target(pt("dev_center"), { look: { pose: "idle", dir: "f" } });
  t.retarget(center, NOW);
  t.pose({ pose: "clap", dir: "f" }, CLAP_MS, NOW);
  const ms = walkMs(walkPath(fp, SEAT, center.at!));
  expect(t.update(NOW + ms - 1).look.pose).toBe("walk");
  expect(t.update(NOW + ms).look.pose).toBe("clap");
  expect(t.update(NOW + ms + CLAP_MS - 1).look.pose).toBe("clap");
  expect(t.update(NOW + ms + CLAP_MS).look).toBe(center.look);
});

test("결재 대기는 boss_pace 두 자리를 오가며 걷고, 다른 자리가 오면 그 자리에서 끊는다", () => {
  const t = track();
  const a = pt("boss_pace", 0);
  const b = pt("boss_pace", 1);
  t.reset(target(a, { pace: true }), NOW);
  const ms = walkMs([a, b]);
  expect(t.update(NOW).look.pose).toBe("walk");
  expect(t.update(NOW + ms).at).toEqual(b);
  expect(t.update(NOW + 2 * ms).at).toEqual(a);
  // 가운데쯤에서 좌석으로
  const seat: Pt = { gx: 20.9, gy: 2.15, z: 0 };
  t.retarget(target(seat), NOW + 2 * ms + ms / 2);
  const s = t.update(NOW + 2 * ms + ms / 2);
  expect(s.at!.gx).toBeGreaterThan(a.gx);
  expect(s.at!.gx).toBeLessThan(b.gx);
  expect(t.update(NOW + 10 * ms).at).toEqual(seat);
});

// ---------- 유휴 동작 ----------

test("유휴 간격은 5~10초, agent_id 해시로 직원마다 다르다", () => {
  for (const seed of [0, 1, 4999, 5000, 5001, 123456789, 0xffffffff]) {
    expect(idlePeriod(seed)).toBeGreaterThanOrEqual(5000);
    expect(idlePeriod(seed)).toBeLessThanOrEqual(10000);
  }
  expect(idlePeriod(1)).not.toBe(idlePeriod(2));
});

test("좌석에 앉은 직원은 간격마다 한두 타일 오갔다 돌아온다", () => {
  const seed = 1234;
  const t = track({ seed });
  const tgt = target(SEAT, { wander: true });
  t.reset(tgt, NOW);
  const p = idlePeriod(seed);
  expect(t.update(NOW + p - 1).at).toEqual(SEAT);
  expect(t.update(NOW + p).look.pose).toBe("walk");
  // 오간 거리는 한두 타일 (gx 로)
  let far = 0;
  for (let ms = 0; ms <= 2 * WALK_MAX_MS; ms += 10) {
    const s = t.update(NOW + p + ms);
    expect(s.at!.gy).toBe(SEAT.gy);
    far = Math.max(far, Math.abs(s.at!.gx - SEAT.gx));
  }
  expect(far).toBeGreaterThanOrEqual(1);
  expect(far).toBeLessThanOrEqual(2);
  // 한 번 오가는 데 1초 남짓, 그 뒤엔 다시 제자리
  const back = t.update(NOW + p + 2 * WALK_MAX_MS);
  expect(back.at).toEqual(SEAT);
  expect(back.look).toBe(tgt.look);
});

test("허가 대기·결재 대기·장애 상황실·졸음·좌석 밖은 유휴 동작을 하지 않는다", () => {
  expect(canWander("Dev", "Type", true)).toBe(true);
  expect(canWander("TeamLead", "Think", true)).toBe(true);
  expect(canWander("Door", "Sit", true)).toBe(false);
  expect(canWander("TeamLead", "Pace", true)).toBe(false);
  expect(canWander("Incident", "Sit", true)).toBe(false);
  expect(canWander("Dev", "Sleep", true)).toBe(false);
  expect(canWander("Test", "Sit", false)).toBe(false);
  // wander 가 꺼진 자리는 오래 지나도 그대로
  const t = track();
  t.reset(target(pt("door_wait")), NOW);
  expect(t.update(NOW + 60_000).at).toEqual(pt("door_wait"));
});

// ---------- 움직임 줄이기 ----------

test("prefers-reduced-motion 이면 걷지 않고 즉시 이동, 왕복·유휴 동작도 없다", () => {
  const t = track({ reduced: true });
  t.reset(target(SEAT, { wander: true }), NOW);
  t.retarget(target(pt("library")), NOW);
  expect(t.update(NOW).at).toEqual(pt("library"));
  t.retarget(target(SEAT), NOW, true);
  expect(t.update(NOW).at).toEqual(SEAT);
  t.retarget(HIDDEN, NOW);
  expect(t.update(NOW).at).toBeNull();
  const a = pt("boss_pace", 0);
  t.reset(target(a, { pace: true }), NOW);
  expect(t.update(NOW + 5000).at).toEqual(a);
  t.reset(target(SEAT, { wander: true }), NOW);
  expect(t.update(NOW + 60_000).at).toEqual(SEAT);
});
