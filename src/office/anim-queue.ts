// 직원별 연출 큐: 자리 이동 걷기, cue 재생, 건너뛰기, 결재 대기 왕복, 유휴 동작 (PLAN 4.9, HANDOFF 5·6.4·7).
// 자리는 스냅샷이 정한다. 큐는 "어떻게 거기 갔는지"만 꾸민다 (PLAN 4.8).
import type { CueDto, Pose, Room } from "../types/ipc.ts";
import type { Look } from "./actors.ts";
import { gridToScreen, roomAt, type Floorplan } from "./floorplan.ts";

export const WALK_MIN_MS = 500;
export const WALK_MAX_MS = 1500;
export const ENTER_MS = 4000;
export const CLAP_MS = 1000;
export const REJECT_HOLD_MS = 4000;
export const SCRATCH_MS = 2000;
export const JUMP_MS = 400;
export const RING_MS = 1000;
export const STAMP_MS = 1000;
/** 사장님이 부장 책상 앞에 도착한 뒤 서 있는 시간 */
export const VISIT_STAY_MS = 5000;
/** 이보다 많이 쌓이면 중간을 건너뛰고 마지막 상태로 점프 */
export const MAX_STEPS = 5;
// 방 사이 거리(약 100~390px, 중앙값 250px)가 0.5~1.5초에 고르게 들어오는 속도.
// 자리 주변 한두 타일(13~27px)과 boss_pace 왕복(36px)은 0.5초로 올려 잡는다
const WALK_PX_PER_S = 260;
const CLAP: Look = { pose: "clap", dir: "f" };
const IDLE_LOOK: Look = { pose: "idle", dir: "f" };
const SCRATCH: Look = { pose: "scratch", dir: "f" };
export const JUMP: Look = { pose: "jump", dir: "f" };

export interface Pt {
  gx: number;
  gy: number;
  z: number;
}

/** 스냅샷이 정한 자리. `at` 이 null 이면 숨김 (외근·퇴근·퇴장) */
export interface Target {
  at: Pt | null;
  look: Look;
  depth: number;
  wander: boolean;
  pace: boolean;
  /** 앉는 가구 자리. 이 자리에서 떠나는 첫 구간·도착하는 끝 구간은 가구 depth 를 지켜 앞뒤가 튀지 않게 한다 */
  sitOn: boolean;
}

/** 지금 그릴 모습 */
export interface Shown {
  at: Pt | null;
  look: Look;
  flip: boolean;
  depth: number;
}

type Step =
  | { kind: "walk"; path: Pt[]; ms: number; auto: boolean; head?: number; tail?: number }
  | { kind: "pose"; look: Look; ms: number }
  | { kind: "hide" };

export interface TrackContext {
  fp: Floorplan;
  /** agent_id 해시. 유휴 간격과 방향을 직원마다 어긋나게 */
  seed: number;
  reduced: () => boolean;
}

export const HIDDEN: Target = {
  at: null,
  look: IDLE_LOOK,
  depth: 0,
  wander: false,
  pace: false,
  sitOn: false,
};

// ---------- 경로 ----------

const same = (a: Pt | null, b: Pt | null) =>
  a === b || (!!a && !!b && a.gx === b.gx && a.gy === b.gy && a.z === b.z);

export function stand(fp: Floorplan, name: string, i = 0): Pt {
  const [gx, gy, z = 0] = fp.stands[name][i];
  return { gx, gy, z };
}

const roomIn = (fp: Floorplan, p: Pt) => roomAt(fp, p.gx, p.gy) ?? "dev";

/**
 * floorplan.path 규칙: 현재 위치 → 현재 방 entry → 개발실 통로(dev entry) → 목표 방 entry → 목표.
 * 같은 방이면 직선 하나. A* 없이 직선 구간만 (PLAN 4.9).
 */
export function walkPath(fp: Floorplan, from: Pt, to: Pt): Pt[] {
  const a = roomIn(fp, from);
  const b = roomIn(fp, to);
  const pts = [from];
  if (a !== b)
    // 한쪽 끝이 개발실이면 통로를 거치지 않고 반대쪽 방 entry 로 바로 간다 (거치면 개발실 끝을 찍고 돌아오는 질주가 된다)
    for (const k of a === "dev" ? [b] : b === "dev" ? [a] : [a, "dev", b]) {
      const [gx, gy] = fp.rooms[k].entry;
      pts.push({ gx, gy, z: 0 });
    }
  pts.push(to);
  return pts.filter((p, i) => i === 0 || !same(p, pts[i - 1]));
}

function screen(p: Pt) {
  return gridToScreen(p.gx, p.gy, p.z);
}

function segLen(a: Pt, b: Pt): number {
  const p = screen(a);
  const q = screen(b);
  return Math.hypot(q.x - p.x, q.y - p.y);
}

// 0.5–1.5초로 자른다 (HANDOFF 7)
export function walkMs(path: Pt[]): number {
  let px = 0;
  for (let i = 1; i < path.length; i++) px += segLen(path[i - 1], path[i]);
  return Math.min(WALK_MAX_MS, Math.max(WALK_MIN_MS, (px / WALK_PX_PER_S) * 1000));
}

// 화면 아래로 가면 f, 위로 가면 b, 왼쪽으로 가면 뒤집는다 (HANDOFF 6.4)
function facing(a: Pt, b: Pt): { dir: "f" | "b"; flip: boolean } {
  const p = screen(a);
  const q = screen(b);
  return { dir: q.y < p.y ? "b" : "f", flip: q.x < p.x };
}

// ---------- 유휴 동작 ----------

/**
 * 큐가 비고 자기 좌석에 있는 직원만. 사장님 손이 필요한 상태(허가 대기·결재 대기)와 장애 상황실은 하지 않는다 (PLAN 4.9).
 * 조는 직원은 자리를 뜨면 Zzz 가 깜빡여 뺀다.
 */
export function canWander(room: Room, pose: Pose, seated: boolean): boolean {
  return seated && room !== "Door" && room !== "Incident" && pose !== "Pace" && pose !== "Sleep";
}

/** 5~10초. 직원마다 다르다 */
export function idlePeriod(seed: number): number {
  return 5000 + (seed % 5001);
}

function wanderPoint(fp: Floorplan, at: Pt, seed: number, round: number): Pt {
  const k = (seed ^ Math.imul(round + 1, 0x9e3779b1)) >>> 0;
  const tiles = 1 + (k & 1);
  const sign = k & 2 ? 1 : -1;
  const [x0, , x1] = fp.rooms[roomIn(fp, at)].rect;
  return { ...at, gx: Math.min(x1 - 0.5, Math.max(x0 + 0.5, at.gx + sign * tiles)) };
}

// ---------- 큐 ----------

export class Track {
  private steps: Step[] = [];
  private t0 = 0;
  /** 맨 앞 단계가 시작하는 자리. null 이면 숨어 있다 */
  private pos: Pt | null = null;
  private target: Target = HIDDEN;
  private nextIdle = 0;
  private round = 0;

  constructor(private ctx: TrackContext) {}

  /** 큐를 비우고 바로 제자리 (조용한 재구성, 사무실 전환, 숨은 창에서 돌아옴, 건너뛰기) */
  reset(target: Target, now: number): void {
    this.steps = [];
    this.pos = target.at;
    this.target = target;
    this.t0 = now;
    this.nextIdle = now + idlePeriod(this.ctx.seed);
  }

  /** 새 스냅샷 자리. `entering` 이면 문에서 4초 걸어 들어온다 (출근·재출근) */
  retarget(target: Target, now: number, entering = false): void {
    this.advance(now);
    const prev = this.target;
    this.target = target;
    // 큐(유휴 동작 빼고)는 늘 앞 목표에서 끝난다. 자리가 그대로면 자세만 바뀐다
    if (!entering && same(prev.at, target.at)) return;
    this.cut(now);
    const fp = this.ctx.fp;
    const from = this.end();
    const tail = target.sitOn ? target.depth : undefined;
    const head = from && prev.sitOn && same(from, prev.at) ? prev.depth : undefined;
    if (entering && target.at)
      this.walk(walkPath(fp, stand(fp, "door"), target.at), ENTER_MS, now, false, { tail });
    else if (from && target.at)
      this.walk(walkPath(fp, from, target.at), undefined, now, false, { head, tail });
    else if (from) {
      // 외근·퇴장: 출구까지 걸어가 사라진다
      this.walk(walkPath(fp, from, stand(fp, "exit")), undefined, now, false, { head });
      this.push({ kind: "hide" }, now);
    } else if (target.at)
      // 숨어 있다 돌아오면 문에서 들어온다 (웹 복귀)
      this.walk(walkPath(fp, stand(fp, "door"), target.at), undefined, now, false, { tail });
    if (this.steps.length > MAX_STEPS) this.reset(target, now);
  }

  /** 제자리에서 자세 하나를 ms 동안 (부장 clap 등). 앞선 걷기가 끝난 뒤 */
  pose(look: Look, ms: number, now: number): void {
    this.advance(now);
    const at = this.target.at;
    if (!at) return;
    // 병렬 Spawn 처럼 같은 자세가 줄줄이 와도 하나로 합쳐 MAX_STEPS 를 넘기지 않는다
    const last = this.steps[this.steps.length - 1];
    if (last?.kind === "pose" && last.look.pose === look.pose) return;
    this.cut(now);
    // 유휴 동작을 끊었으면 자리로 돌아간 뒤에
    const from = this.end();
    if (from && !same(from, at)) this.walk(walkPath(this.ctx.fp, from, at), undefined, now);
    this.push({ kind: "pose", look, ms }, now);
    if (this.steps.length > MAX_STEPS) this.reset(this.target, now);
  }

  /** 반려 직후 사장님 손이 필요한 동안(needs_boss)은 유휴 동작을 미룬다 (PLAN 4.7.2) */
  holdIdle(ms: number, now: number): void {
    this.nextIdle = Math.max(this.nextIdle, now + ms);
  }

  update(now: number): Shown {
    this.advance(now);
    if (this.steps.length === 0) this.idle(now);
    const s = this.steps[0];
    const t = this.target;
    // hide 는 길이가 0 이라 advance 가 이미 걷어 냈다
    const settled = same(this.pos, t.at);
    let at = this.pos;
    let look = settled ? t.look : IDLE_LOOK;
    let flip = false;
    let depth = settled ? t.depth : this.pos ? this.pos.gx + this.pos.gy : 0;
    if (s?.kind === "pose") look = s.look;
    else if (s?.kind === "walk") {
      const w = along(s.path, s.ms > 0 ? (now - this.t0) / s.ms : 1);
      const f = facing(s.path[w.seg], s.path[w.seg + 1] ?? s.path[w.seg]);
      at = w.at;
      look = { pose: "walk", dir: f.dir };
      flip = f.flip;
      // 유휴 동작은 좌석 depth 를 그대로 써야 칸막이·책상과 앞뒤가 안 뒤집힌다
      if (s.auto && t.wander) depth = t.depth;
      else if (w.seg === 0 && s.head !== undefined) depth = s.head;
      else if (w.seg === s.path.length - 2 && s.tail !== undefined) depth = s.tail;
      else depth = at.gx + at.gy;
    }
    return { at, look, flip, depth };
  }

  // 끝난 단계를 걷어 낸다. 다음 단계는 앞 단계가 끝난 시각에 시작해 숨은 창의 긴 틱도 한 번에 따라잡는다
  private advance(now: number): void {
    while (this.steps.length) {
      const s = this.steps[0];
      const end = this.t0 + (s.kind === "hide" ? 0 : s.ms);
      if (now < end) return;
      if (s.kind === "walk") this.pos = s.path[s.path.length - 1];
      else if (s.kind === "hide") this.pos = null;
      this.steps.shift();
      this.t0 = end;
      if (this.steps.length === 0) this.nextIdle = end + idlePeriod(this.ctx.seed);
    }
  }

  /** 큐가 다 돌면 설 자리 */
  private end(): Pt | null {
    let p = this.pos;
    for (const s of this.steps)
      if (s.kind === "walk") p = s.path[s.path.length - 1];
      else if (s.kind === "hide") p = null;
    return p;
  }

  // 유휴 동작·왕복은 진짜 단계가 오면 그 자리에서 끊는다
  private cut(now: number): void {
    const s = this.steps[0];
    if (s?.kind !== "walk" || !s.auto) return;
    this.pos = along(s.path, (now - this.t0) / s.ms).at;
    this.steps = [];
    this.t0 = now;
  }

  private push(s: Step, now: number): void {
    if (this.steps.length === 0) this.t0 = now;
    this.steps.push(s);
  }

  private walk(
    path: Pt[],
    ms: number | undefined,
    now: number,
    auto = false,
    depths: { head?: number; tail?: number } = {},
  ): void {
    // 움직임 줄이기: 걷지 않고 즉시 이동 (HANDOFF 7)
    const t = this.ctx.reduced() ? 0 : (ms ?? walkMs(path));
    this.push({ kind: "walk", path, ms: t, auto, ...depths }, now);
  }

  private idle(now: number): void {
    const t = this.target;
    const at = this.pos;
    if (!at || !t.at || this.ctx.reduced()) return;
    const fp = this.ctx.fp;
    if (t.pace) {
      const a = stand(fp, "boss_pace", 0);
      const b = stand(fp, "boss_pace", 1);
      this.walk([at, same(at, b) ? a : b], undefined, now, true);
    } else if (t.wander && same(at, t.at) && now >= this.nextIdle) {
      const w = wanderPoint(fp, at, this.ctx.seed, this.round++);
      this.walk([at, w], undefined, now, true);
      this.walk([w, at], undefined, now, true);
    }
  }
}

/** 경로 위 비율 f(0~1) 지점과 그 구간 번호. 구간마다 화면 거리로 나눈다 */
function along(path: Pt[], f: number): { at: Pt; seg: number } {
  const lens = path.slice(1).map((p, i) => segLen(path[i], p));
  const total = lens.reduce((a, b) => a + b, 0);
  let d = Math.min(1, Math.max(0, f)) * total;
  for (let i = 0; i < lens.length; i++) {
    if (d <= lens[i] && lens[i] > 0) {
      const k = d / lens[i];
      const a = path[i];
      const b = path[i + 1];
      return {
        at: {
          gx: a.gx + (b.gx - a.gx) * k,
          gy: a.gy + (b.gy - a.gy) * k,
          z: a.z + (b.z - a.z) * k,
        },
        seg: i,
      };
    }
    d -= lens[i];
  }
  return { at: path[path.length - 1], seg: Math.max(0, path.length - 2) };
}

// ---------- cue ----------

/**
 * cue 하나를 그 직원 큐에 (팀 단위 cue 는 track 없이). Enter·Rehire·Retire 는 자리 이동(retarget)이 이미 걷게 했다.
 * 사장님 방문(BossVisit)은 직원 큐가 아니라 ActorLayer 가 액터를 만들어 한다.
 * ponytail: 도장·트럭·사이렌 같은 나머지 연출은 아직 없어 무시한다, T25~T26 이 여기에 넣는다
 */
export function playCue(cue: CueDto, track: Track | undefined, now: number): void {
  if (cue.kind === "Clap") track?.pose(CLAP, CLAP_MS, now);
  else if (cue.kind === "Reject") {
    track?.holdIdle(REJECT_HOLD_MS, now);
    track?.pose(SCRATCH, SCRATCH_MS, now);
  }
}
