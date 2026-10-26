import { expect, test } from "vite-plus/test";
import rawFloorplan from "../../design/sprites/floorplan.json";
import type { CueDto } from "../types/ipc.ts";
import {
  CLAP_MS,
  ENTER_MS,
  HIDDEN,
  MAX_STEPS,
  REJECT_HOLD_MS,
  SCRATCH_MS,
  Track,
  WALK_MAX_MS,
  WALK_MIN_MS,
  canWander,
  idlePeriod,
  playCue,
  walkPath,
  type Pt,
  type Shown,
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
  return {
    at,
    look: SIT,
    depth: at ? at.gx + at.gy : 0,
    wander: false,
    pace: false,
    sitOn: false,
    ...o,
  };
}

function track(o: { seed?: number; reduced?: boolean } = {}) {
  return new Track({ fp, seed: o.seed ?? 0, reduced: () => o.reduced ?? false });
}

/** 5ms 간격으로 시각을 올려 가며 조건이 처음 맞는 시각을 찾는다 (update 는 시각이 앞으로만 간다) */
function until(t: Track, from: number, ok: (s: Shown) => boolean): number {
  for (let now = from; now < from + 10_000; now += 5) if (ok(t.update(now))) return now;
  throw new Error("조건이 맞는 시각이 없다");
}
const walking = (s: Shown) => s.look.pose === "walk";
const notWalking = (s: Shown) => !walking(s);

// ---------- 경로·시간·방향 ----------

test("같은 방은 직선 하나, 다른 방은 방 entry 를 지나고 개발실 통로는 양쪽 다 다른 방일 때만 거친다", () => {
  const a = pt("test", 0);
  const b = pt("test", 1);
  expect(walkPath(fp, a, b)).toEqual([a, b]);
  // 한쪽 끝이 개발실이면 통로(dev entry)를 건너뛰고 반대쪽 방 entry 로 바로 간다
  const to = pt("test");
  expect(walkPath(fp, SEAT, to)).toEqual([SEAT, entry("test"), to]);
  expect(walkPath(fp, to, SEAT)).toEqual([to, entry("test"), SEAT]);
  // 테스트실 → 서고: 나가는 방 entry → 통로 → 들어가는 방 entry
  const lib = pt("library");
  expect(walkPath(fp, to, lib)).toEqual([to, entry("test"), entry("dev"), entry("library"), lib]);
});

test("걷는 시간은 1–3초로 자른다", () => {
  const t = track();
  t.reset(target(SEAT), NOW);
  t.retarget(target({ ...SEAT, gx: SEAT.gx + 1 }), NOW);
  expect(walking(t.update(NOW + 999))).toBe(true);
  expect(walking(t.update(NOW + 1000))).toBe(false);
  // 가장 먼 방 사이 걷기도 3초를 넘지 않는다
  const far = track();
  far.reset(target(pt("door")), NOW);
  far.retarget(target(pt("boss_window")), NOW);
  expect(walking(far.update(NOW + 2999))).toBe(true);
  expect(walking(far.update(NOW + 3000))).toBe(false);
});

test("화면 왼쪽으로 걸을 때만 뒤집는다", () => {
  const o: Pt = { gx: 5, gy: 5, z: 0 };
  const flipOf = (to: Pt) => {
    const t = track();
    t.reset(target(o), NOW);
    t.retarget(target(to), NOW);
    return t.update(NOW + 1).flip;
  };
  // gx+ 는 화면 오른쪽 아래, gy+ 는 왼쪽 아래
  expect(flipOf({ ...o, gx: 6 })).toBe(false);
  expect(flipOf({ ...o, gy: 6 })).toBe(true);
});

// ---------- 큐 ----------

test("자리가 바뀌면 walk 로 걷고, 도착하면 그 자리 자세로 반전 없이 선다", () => {
  const to = pt("library");
  const tgt = target(to, { look: { pose: "carry", dir: "f", held: "folder" } });
  const run = () => {
    const t = track();
    t.reset(target(pt("test")), NOW);
    t.retarget(tgt, NOW);
    return t;
  };
  const ms = until(run(), NOW, notWalking) - NOW;
  const t = run();
  const end = NOW + ms;
  // 첫 구간은 테스트실 안 왼쪽 아래
  const s0 = t.update(NOW + 1);
  expect(s0.look).toEqual({ pose: "walk", dir: "f" });
  expect(s0.flip).toBe(true);
  // 개발실 통로 → 서고 entry 는 왼쪽 위 (경로 237px 중 83~220px 구간)
  const s1 = t.update(NOW + ms * 0.6);
  expect(s1.look).toEqual({ pose: "walk", dir: "b" });
  expect(s1.flip).toBe(true);
  // entry → 서가 앞은 오른쪽 위
  const s2 = t.update(end - 10);
  expect(s2.look).toEqual({ pose: "walk", dir: "b" });
  expect(s2.flip).toBe(false);
  const done = t.update(end);
  expect(done.at).toEqual(to);
  expect(done.look).toBe(tgt.look);
  expect(done.flip).toBe(false);
  expect(done.depth).toBe(tgt.depth);
});

test("걷는 중에 온 다음 자리는 앞 걷기가 끝난 곳에서 이어 걷는다", () => {
  const t = track();
  t.reset(target(SEAT), NOW);
  t.retarget(target(pt("test")), NOW);
  t.retarget(target(pt("build")), NOW + 100);
  // 첫 걷기가 끝난 순간 테스트실에 있다
  // 5ms 간격 표본이라 0.15칸 안이면 닿은 것으로 본다
  const near = (p: Pt) => (s: Shown) =>
    Math.abs(s.at!.gx - p.gx) < 0.15 && Math.abs(s.at!.gy - p.gy) < 0.15;
  const t1 = until(t, NOW, near(pt("test")));
  expect(walking(t.update(t1))).toBe(true);
  const t2 = until(t, t1 + 50, near(pt("build")));
  expect(t.update(until(t, t2, notWalking)).at).toEqual(pt("build"));
});

test(`큐가 ${MAX_STEPS}개 넘게 쌓이면 중간을 건너뛰고 마지막 자리로 점프`, () => {
  const t = track();
  t.reset(target(SEAT), NOW);
  const spots = [pt("test"), pt("build"), pt("library"), pt("meeting"), pt("shipping")];
  for (const s of spots) t.retarget(target(s), NOW);
  // 다섯 단계까지는 아직 출발 자리에서 걷기 시작
  expect(t.update(NOW).at).toEqual(SEAT);
  const last = target(pt("review_sofa"));
  t.retarget(last, NOW);
  expect(t.update(NOW).at).toEqual(last.at);
});

test("reset 은 큐를 비우고 바로 제자리 (조용한 재구성·사무실 전환)", () => {
  const t = track();
  t.reset(target(SEAT), NOW);
  t.retarget(target(pt("test")), NOW);
  const to = target(pt("library"));
  t.reset(to, NOW + 10);
  const s = t.update(NOW + 10);
  expect(s.at).toEqual(to.at);
  expect(s.look).toBe(to.look);
});

test("출근은 문에서 좌석까지 6초 걷는다", () => {
  const t = track();
  t.retarget(target(SEAT), NOW, true);
  expect(t.update(NOW).at).toEqual(pt("door"));
  const mid = t.update(NOW + 5999);
  expect(mid.look.pose).toBe("walk");
  expect(t.update(NOW + 6000).at).toEqual(SEAT);
});

test("숨기면 출구까지 걸어가 사라지고, 다시 보이면 문에서 들어온다", () => {
  const t = track();
  t.reset(target(SEAT), NOW);
  t.retarget(HIDDEN, NOW);
  expect(t.update(NOW + 1).at).not.toBeNull();
  const gone = until(t, NOW, (s) => s.at === null);
  expect(gone - NOW).toBeGreaterThanOrEqual(WALK_MIN_MS);
  // 웹 복귀
  t.retarget(target(SEAT), gone);
  expect(t.update(gone).at).toEqual(pt("door"));
});

const clap = () => ({ teamId: "t", kind: "Clap", agent: "a" }) as CueDto;

test("clap 은 앞 걷기가 끝난 뒤 1초, 그다음 자리 자세", () => {
  const t = track();
  t.reset(target(SEAT), NOW);
  const center = target(pt("dev_center"), { look: { pose: "idle", dir: "f" } });
  t.retarget(center, NOW);
  playCue(clap(), t, NOW);
  // 가까운 걷기라 1초
  expect(t.update(NOW + WALK_MIN_MS - 1).look.pose).toBe("walk");
  expect(t.update(NOW + WALK_MIN_MS).look.pose).toBe("clap");
  expect(t.update(NOW + WALK_MIN_MS + CLAP_MS - 1).look.pose).toBe("clap");
  expect(t.update(NOW + WALK_MIN_MS + CLAP_MS).look).toBe(center.look);
});

test("clap 이 줄줄이 와도(병렬 Spawn) 하나로 합쳐 손뼉을 치고 순간이동하지 않는다", () => {
  const t = track();
  t.reset(target(SEAT), NOW);
  const center = target(pt("dev_center"), { look: { pose: "idle", dir: "f" } });
  t.retarget(center, NOW);
  for (let i = 0; i < MAX_STEPS; i++) playCue(clap(), t, NOW);
  expect(t.update(NOW).at).toEqual(SEAT);
  const c0 = until(t, NOW, (s) => s.look.pose === "clap");
  expect(t.update(c0 + CLAP_MS).look).toBe(center.look);
});

test("결재 대기는 boss_pace 두 자리를 오가며 걷고, 다른 자리가 오면 그 자리에서 끊는다", () => {
  const t = track();
  const a = pt("boss_pace", 0);
  const b = pt("boss_pace", 1);
  t.reset(target(a, { pace: true }), NOW);
  const ms = WALK_MIN_MS; // 36px 왕복
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
    // 오가는 동안에도 좌석 depth 그대로 (칸막이·책상과 앞뒤가 안 뒤집힌다)
    expect(s.depth).toBe(tgt.depth);
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

test("반려 cue 를 받으면 부장이 머리를 긁고 좌석으로, 다음 유휴 동작은 4초 넘게 밀린다", () => {
  const seed = 1234;
  const p = idlePeriod(seed);
  const t = track({ seed });
  t.reset(target(SEAT, { wander: true }), NOW);
  const reject = { teamId: "t", kind: "Reject", agent: "a" } as CueDto;
  const at = NOW + p - 1000;
  playCue(reject, t, at);
  expect(t.update(at).look.pose).toBe("scratch");
  expect(t.update(at + SCRATCH_MS - 1).look.pose).toBe("scratch");
  expect(t.update(at + SCRATCH_MS).look.pose).toBe("sit");
  expect(t.update(at + REJECT_HOLD_MS - 1).look.pose).not.toBe("walk");
  expect(t.update(at + REJECT_HOLD_MS).look.pose).toBe("sit");
});

test("앉는 가구 자리에서 떠나는 첫 구간과 도착하는 끝 구간은 가구 depth 를 지킨다", () => {
  const sofa = pt("review_sofa");
  const SOFA_DEPTH = 99;
  const leave = () => {
    const t = track();
    t.reset(target(sofa, { sitOn: true, depth: SOFA_DEPTH }), NOW);
    t.retarget(target(SEAT), NOW);
    return t;
  };
  const gone = until(leave(), NOW, notWalking);
  const t = leave();
  // 소파 → review entry → 좌석: 첫 구간(전체의 18%)만 소파 depth
  const first = t.update(NOW + (gone - NOW) * 0.05);
  expect(first.depth).toBe(SOFA_DEPTH);
  const mid = t.update(NOW + (gone - NOW) * 0.6);
  expect(mid.depth).toBe(mid.at!.gx + mid.at!.gy);
  // 돌아올 때는 끝 구간만
  t.retarget(target(sofa, { sitOn: true, depth: SOFA_DEPTH }), gone);
  const start = t.update(gone);
  expect(start.depth).toBe(start.at!.gx + start.at!.gy);
  const back = until(t, gone, notWalking);
  expect(t.update(back - 10).depth).toBe(SOFA_DEPTH);
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

test("걷는 중 움직임 줄이기를 켜면 현재 논리 자리에 즉시 멈추고 큐와 출입 소품은 다시 나오지 않는다", () => {
  let reduced = false;
  const t = new Track({ fp, seed: 0, reduced: () => reduced });
  const destination = target(pt("library"));
  t.reset(target(SEAT), NOW);
  t.retarget(destination, NOW, true, "flashlight");
  playCue({ kind: "Clap", teamId: "t", agent: "a" }, t, NOW + 1);
  expect(t.update(NOW + 100).look).toMatchObject({ pose: "walk", held: "flashlight" });
  reduced = true;
  expect(t.update(NOW + 101)).toMatchObject({
    at: destination.at,
    look: SIT,
    depth: destination.depth,
    flip: false,
  });
  reduced = false;
  for (const dt of [102, ENTER_MS, ENTER_MS + CLAP_MS + 100])
    expect(t.update(NOW + dt)).toMatchObject({ at: destination.at, look: SIT });
});

test("퇴근 걷기 중 움직임 줄이기는 즉시 숨기고 다시 켜도 서류 가방·걷기를 재생하지 않는다", () => {
  let reduced = false;
  const t = new Track({ fp, seed: 0, reduced: () => reduced });
  t.reset(target(SEAT), NOW);
  t.retarget(HIDDEN, NOW, false, "briefcase");
  expect(t.update(NOW + 100).look).toMatchObject({ pose: "walk", held: "briefcase" });
  reduced = true;
  expect(t.update(NOW + 101).at).toBeNull();
  reduced = false;
  const shown = t.update(NOW + 102);
  expect(shown.at).toBeNull();
  expect(shown.look.held).toBeUndefined();
  expect(t.update(NOW + WALK_MAX_MS + 100).at).toBeNull();
});

for (const decoration of ["highfive", "errand"] as const)
  test(`${decoration} 도중 움직임 줄이기는 임시 목적지가 아닌 논리 복귀 자리에 멈춘다`, () => {
    let reduced = false;
    const t = new Track({ fp, seed: 0, reduced: () => reduced });
    t.reset(target(SEAT), NOW);
    if (decoration === "highfive") t.highfive(pt("dev_center"), WALK_MAX_MS, true, NOW);
    else t.errand(pt("library"), { pose: "phone", dir: "f" }, 1000, NOW);
    expect(walking(t.update(NOW + 100))).toBe(true);
    reduced = true;
    expect(t.update(NOW + 101)).toMatchObject({ at: SEAT, look: SIT, flip: false });
    reduced = false;
    expect(t.update(NOW + 5000)).toMatchObject({ at: SEAT, look: SIT, flip: false });
  });
