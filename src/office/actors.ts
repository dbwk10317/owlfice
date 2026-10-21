// 직원 액터: 스냅샷의 직원을 방·좌석에 세우고 세 겹 올빼미로 그린다 (HANDOFF 4.4, 5, 6.4).
import { Container, Graphics, Rectangle, Sprite, type Spritesheet, type Texture } from "pixi.js";
import phrases from "../../design/handoff/phrases.ko.json";
import type { Bubble, CueDto, EmployeeDto, OfficeDto, Pose, Room, TeamDto } from "../types/ipc.ts";
import { reducedMotion } from "../loading.ts";
import { fill, fillElapsed } from "../text.ts";
import { empKey } from "../ticker.ts";
import {
  HIDDEN,
  JUMP,
  JUMP_MS,
  STAMP_MS,
  Track,
  VISIT_STAY_MS,
  canWander,
  playCue,
  stand,
  walkMs,
  walkPath,
  type Target,
} from "./anim-queue.ts";
import { gridToScreen, themeOf, zIndexOf, type Floorplan, type Kind } from "./floorplan.ts";
import { placeSprite } from "./sprites.ts";

const BLINK_MS = 150;
const ZZ_OFFSET: [number, number] = [6, -10];
const BADGE_OFFSET: [number, number] = [14, 6];
const BUBBLE_MS = 4000;
/** 사장님 말풍선은 30자에서 자른다 (HANDOFF 5절) */
const BOSS_TEXT_MAX = 30;
/** 사장님이 부장 좌석에서 떨어져 서는 거리 (gy 칸) */
const VISIT_FRONT = 2;

export interface OwlMeta {
  anchor: [number, number];
  employeeSpecies: string[];
  poses: Record<string, { dirs: Record<string, number>; fps: number }>;
  anchors: Record<string, { hand: [number, number]; head: [number, number] }>;
}

/** owls.json 의 텍스처와 `owlfice` 메타. 로드에 실패하면 null 로 받아 대체 몸체를 그린다. */
export interface Owls {
  textures: Record<string, Texture>;
  meta: OwlMeta;
}

// ---------- 종 ----------

/** 백엔드 names.rs 의 fnv1a64 와 같다 (UTF-8 바이트). 종을 백엔드 이름과 같은 해시로 골라야 한다 (HANDOFF 6.4). */
export function fnv1a64(s: string): bigint {
  let h = 0xcbf29ce484222325n;
  for (const b of new TextEncoder().encode(s))
    h = ((h ^ BigInt(b)) * 0x100000001b3n) & 0xffffffffffffffffn;
  return h;
}

export function speciesOf(agentId: string, species: string[]): string {
  return species[Number(fnv1a64(agentId) % BigInt(species.length))];
}

// ---------- 자리와 자세 (HANDOFF 5절) ----------

export interface Look {
  pose: string;
  dir: "f" | "b";
  held?: string;
}

interface Rule {
  /** "seat" 는 자기 좌석, 아니면 floorplan.stands 이름을 이은 목록 */
  at: "seat" | string[];
  look: Look;
  /** 서 있는 자리 이름별로 자세가 다를 때 (탕비실: 콘솔 sit, 안마의자 sleep, 그 밖 idle) */
  looks?: Record<string, Look>;
  /** 목록에서 먼저 볼 자리. 장애 상황실 0번은 부장 몫이다 */
  from?: number;
}

const IDLE: Look = { pose: "idle", dir: "f" };
const SIT: Look = { pose: "sit", dir: "f" };

const SEAT_LOOK: Record<Exclude<Pose, "Pace">, Look> = {
  Sit: SIT,
  Type: { pose: "type", dir: "f" },
  Read: { pose: "read", dir: "f" },
  // 창 쪽으로 돌아앉음
  Think: { pose: "idle", dir: "b" },
  Duck: { pose: "carry", dir: "f", held: "duck" },
  Squat: { pose: "crouch", dir: "f" },
  Sleep: { pose: "sleep", dir: "f" },
};

// 백엔드 Room 하나에 HANDOFF 5절 행이 여럿이면(셸·빌드, git 조회·큰 열람) DTO 로 가를 수 없어 자리 목록을 잇는다
const ROOM_RULE: Partial<Record<Room, Rule>> = {
  Archive: { at: ["library"], look: { pose: "carry", dir: "f", held: "folder" } },
  Build: { at: ["build_terminal", "build"], look: IDLE },
  Test: { at: ["test"], look: IDLE },
  Review: { at: ["review_sofa"], look: SIT },
  Meeting: { at: ["meeting"], look: IDLE },
  Shipping: { at: ["shipping"], look: { pose: "carry", dir: "f", held: "box_carry" } },
  Door: { at: ["door_wait", "door"], look: { pose: "carry", dir: "f", held: "clipboard" } },
  // 서브 종료 (HANDOFF 5절): 첫째 콘솔, 둘째 안마의자, 그다음은 탕비실에 서 있는다
  Pantry: {
    at: ["pantry_console", "pantry_massage", "pantry"],
    look: SIT,
    looks: { pantry_console: SIT, pantry_massage: SEAT_LOOK.Sleep, pantry: IDLE },
  },
  // 백엔드는 팀원을 장애 상황실로 보내지 않지만 표대로 둔다
  Incident: { at: ["incident"], look: IDLE, from: 1 },
  // 부장 아닌 직원이 팀장실, 부장이 개발실에 있을 때
  TeamLead: { at: ["boss_pace"], look: IDLE },
  Dev: { at: ["dev_center"], look: IDLE },
};

/** 방·자세 → 자리와 sprite 자세. null 은 그리지 않는다 (외근). */
export function placeRule(room: Room, pose: Pose, boss: boolean): Rule | null {
  if (room === "Outside") return null;
  if (pose === "Pace") return { at: ["boss_pace"], look: IDLE };
  if (room === (boss ? "TeamLead" : "Dev")) return { at: "seat", look: SEAT_LOOK[pose] };
  if (room === "Incident" && boss) return { at: ["incident"], look: { pose: "panic", dir: "f" } };
  if (room === "Pantry" && pose === "Sleep")
    return { at: ["pantry_sofa", "pantry_massage"], look: SEAT_LOOK.Sleep };
  const rule = ROOM_RULE[room];
  if (!rule) return null;
  if (pose === "Squat" || pose === "Sleep")
    return { ...rule, looks: undefined, look: SEAT_LOOK[pose] };
  return rule;
}

// 앉는 자리는 가구와 같은 depth 에 두고 순번으로 가구 뒤에 그린다. gx + gy 로는 방석·소파 앞뒤가 뒤집힌다
const SIT_ON: Record<string, string> = {
  review_sofa: "review_seat",
  pantry_sofa: "sofa",
  pantry_massage: "massage_chair",
};

interface Spot {
  id: string;
  gx: number;
  gy: number;
  z: number;
  depth: number;
  desk?: string;
  look?: Look;
}

export interface Placed {
  emp: EmployeeDto;
  team: TeamDto;
  spot: string;
  x: number;
  y: number;
  gx: number;
  gy: number;
  z: number;
  depth: number;
  look: Look;
  seated: boolean;
}

/** 참조 렌더(Python round)와 같은 은행가 반올림. 의자·소파 자리 몇 곳이 정확히 .5 에 떨어진다. */
export function roundEven(v: number): number {
  const f = Math.floor(v);
  const d = v - f;
  if (d !== 0.5) return Math.round(v);
  return f % 2 === 0 ? f : f + 1;
}

function standSpots(fp: Floorplan, kind: Kind, rule: Rule & { at: string[] }): Spot[] {
  const placements = fp.layouts[kind].placements;
  return rule.at.flatMap((name) =>
    (fp.stands[name] ?? []).map(([gx, gy, z = 0], i) => {
      const on = SIT_ON[name] && placements.find((p) => p.id === SIT_ON[name]);
      return {
        id: `${name}_${i}`,
        gx,
        gy,
        z,
        depth: on ? on.depth : gx + gy,
        look: rule.looks?.[name],
      };
    }),
  );
}

function seatSpots(
  fp: Floorplan,
  kind: Kind,
  team: TeamDto,
  emp: EmployeeDto,
  boss: boolean,
): { spots: Spot[]; index: number } {
  const s = fp.layouts[kind].seats;
  const toSpot = (seat: {
    id: string;
    gx: number;
    gy: number;
    depth: number;
    desk: string;
  }): Spot => ({
    ...seat,
    z: 0,
  });
  if (!boss) {
    const spots = [...s.dev, ...s.temp].map(toSpot);
    // ponytail: 백엔드는 10명째 뒤 직원에게 좌석을 안 준다(카드만). 문서(3.4)대로 마지막 임시 책상에 겹치고 만석 쪽지를 붙인다
    return { spots, index: emp.seat ?? spots.length };
  }
  if (kind === "large") return { spots: s.boss.map(toSpot), index: 0 };
  // 팀장석 0~3, 간이 4~5 (PLAN 4.7.9). deskSlot null 은 layoutActors 가 먼저 거른다
  return { spots: [...s.boss, ...s.spare].map(toSpot), index: team.deskSlot ?? 0 };
}

/**
 * 스냅샷 → 그릴 직원과 자리. 순서는 팀 순서 → 직원 순서 (같은 depth 에서 그리는 순서, HANDOFF 6.2).
 * 같은 자리를 먼저 차지한 직원이 있으면 그 목록의 다음 자리, 다 찼으면 마지막 자리에 겹친다.
 */
export function layoutActors(fp: Floorplan, kind: Kind, office: OfficeDto): Placed[] {
  const used = new Set<string>();
  const out: Placed[] = [];
  for (const team of office.teams) {
    // 퇴근한 팀은 모두 퇴장한다 (HANDOFF 5절)
    if (team.phase === "Off") continue;
    team.employees.forEach((emp, i) => {
      const boss = i === 0;
      // 팀장석·간이 책상이 다 찬 팀은 카드만 (PLAN 3.4). 상황실·탕비실에서도 자리를 차지하지 않는다
      if (boss && kind === "small" && team.deskSlot === null) return;
      const rule = placeRule(emp.activity.room, emp.activity.pose, boss);
      if (!rule) return;
      let spots: Spot[];
      let start: number;
      if (rule.at === "seat") {
        const seat = seatSpots(fp, kind, team, emp, boss);
        spots = seat.spots;
        start = seat.index;
      } else {
        spots = standSpots(fp, kind, { ...rule, at: rule.at });
        start = rule.from ?? 0;
      }
      if (spots.length === 0) return;
      start = Math.min(Math.max(0, start), spots.length - 1);
      let k = start;
      while (k < spots.length && used.has(spots[k].id)) k++;
      // ponytail: 자리가 모자라면 마지막 자리에 겹친다, 겹침 연출은 T29
      const spot = spots[Math.min(k, spots.length - 1)];
      used.add(spot.id);
      const p = gridToScreen(spot.gx, spot.gy, spot.z);
      out.push({
        emp,
        team,
        spot: spot.id,
        x: roundEven(p.x),
        y: roundEven(p.y),
        gx: spot.gx,
        gy: spot.gy,
        z: spot.z,
        depth: spot.depth,
        look: spot.look ?? rule.look,
        seated: rule.at === "seat",
      });
    });
  }
  return out;
}

/**
 * 평면도가 숨겨 둔 소품 중 지금 보여야 할 것: 임시·간이 책상과 쪽지 (HANDOFF 6.5).
 * 가구는 지금 선 자리가 아니라 좌석 배정(seat, deskSlot)으로 정한다. 임시 자리 직원이 다른 방에 가도 책상은 남는다.
 * 만석 쪽지는 임시 책상 쪽지와 같은 자리에 놓고 그것을 대신한다.
 */
export function seatProps(fp: Floorplan, kind: Kind, office: OfficeDto | null): Set<string> {
  const s = fp.layouts[kind].seats;
  const out = new Set<string>();
  let temp = false;
  let full = false;
  for (const team of office?.teams ?? []) {
    // 퇴근한 팀은 모두 퇴장했고 백엔드도 자리를 비운다
    if (team.phase === "Off") continue;
    team.employees.forEach((emp, i) => {
      const boss = i === 0;
      if (boss && kind === "small" && team.deskSlot === null) return;
      // 퇴장한 서브는 백엔드가 자리를 비운다
      if (!boss && emp.retiredAt !== null) return;
      const { spots, index } = seatSpots(fp, kind, team, emp, boss);
      const spot = spots[Math.min(index, spots.length - 1)];
      if (!spot?.desk) return;
      // 팀장석 책상은 평면도에 늘 있다. 켜는 건 간이·임시 책상뿐
      const isTemp = s.temp.some((x) => x.id === spot.id);
      if (!isTemp && !s.spare.some((x) => x.id === spot.id)) return;
      out.add(spot.desk);
      if (isTemp) {
        temp = true;
        full ||= index >= spots.length;
      }
    });
  }
  if (temp) {
    // 접이식 책상은 7명째부터 4자리가 한꺼번에 펴진다 (PLAN 3.4)
    for (const x of s.temp) if (x.desk) out.add(x.desk);
    out.add(full ? "note_full" : "note_temp");
  }
  return out;
}

// ---------- 올빼미 프레임 ----------

/**
 * 세 겹 프레임 이름 (HANDOFF 6.4). 시트에 없는 겹은 null: 수습은 담당 겹이 없고,
 * 담당 소품이 안 보이는 자세(예: 색상표의 type)는 담당 프레임이 없다.
 */
export function owlFrameKeys(
  textures: Record<string, Texture>,
  species: string,
  outfit: string,
  role: string | null,
  look: Look,
  frame: number,
): (string | null)[] {
  const name = `${look.pose}_${look.dir}_${frame}`;
  const keys = [
    `body/${species}/${name}`,
    `outfit/${outfit}/${name}`,
    role && `role/${role}/${name}`,
  ];
  return keys.map((k) => (k && textures[k] ? k : null));
}

// ---------- 말풍선 문구 (PLAN 3.6, 4.8) ----------

const warned = new Set<string>();
function warnOnce(key: string): void {
  if (warned.has(key)) return;
  warned.add(key);
  console.warn(`말풍선 문구 풀에 없는 키: ${key}`);
}

/**
 * 말풍선 종류와 재료 → 글. 풀은 hash(agent_id + at) 로 골라 다시 그려도 같은 문구다.
 * 키가 phrases.bubble 에 없으면 null (그리지 않는다).
 */
export function bubbleText(agentId: string, rank: string, b: Bubble): string | null {
  const pools = phrases.bubble as Record<string, unknown>;
  const say = b.say;
  if (say.kind === "Say") return say.text;
  const key = say.kind[0].toLowerCase() + say.kind.slice(1);
  let pool = pools[key];
  // 직급별 풀: { bujang: [...], ... }
  if (pool && typeof pool === "object" && !Array.isArray(pool))
    pool = (pool as Record<string, unknown>)[rank.toLowerCase()];
  const pick = Array.isArray(pool)
    ? pool[Number(fnv1a64(agentId + b.at) % BigInt(pool.length))]
    : pool;
  if (typeof pick !== "string" || pick === "") {
    warnOnce(say.kind);
    return null;
  }
  return fill(pick, {
    server: "server" in say ? say.server : "",
    tool: "tool" in say ? say.tool : "",
    skill: "skill" in say ? say.skill : "",
  });
}

// ---------- 액터 층 ----------

/** 그리는 데 필요한 것. 사장님 방문 액터는 직원이 아니라 이것만 가진다 */
interface Body {
  /** agent_id. 종·말풍선 문구 해시용 */
  id: string;
  /** actors 맵 키 (empKey) */
  key: string;
  root: Container;
  /** body, outfit, role. 아틀라스가 없으면 비어 있다 */
  layers: Sprite[];
  held: Sprite | null;
  heldName: string | undefined;
  species: string;
  outfit: string;
  role: string | null;
  look: Look;
  spot: string;
  x: number;
  y: number;
  /** 같은 depth 에서 그리는 순번 */
  seq: number;
  track: Track;
  animStart: number;
  nextBlink: number;
  zz: HTMLElement | null;
  badge: HTMLElement | null;
  bubble: HTMLElement | null;
  /** 반려 도장. `stampUntil` 까지 결재판 자리에 보인다 */
  stamp: Sprite | null;
  /** 반려 도장을 기다리는 중: scratch 가 시작되면 그때부터 1초 */
  stampArmed: boolean;
  stampUntil: number;
}

interface Actor extends Body {
  emp: EmployeeDto;
  /** 말풍선 사건 시각(ms). 4초가 지나면 update 가 숨긴다 */
  bubbleAt: number;
}

/** 사장님 방문: 문에서 걸어와 `leaveAt` 에 퇴장한다 (HANDOFF 5절) */
interface Visitor {
  teamId: string;
  body: Body;
  arriveAt: number;
  leaveAt: number;
  leaving: boolean;
}

type ToCss = (x: number, y: number) => { left: number; top: number };

function targetOf(p: Placed): Target {
  const { room, pose } = p.emp.activity;
  return {
    at: { gx: p.gx, gy: p.gy, z: p.z },
    look: p.look,
    depth: p.depth,
    wander: canWander(room, pose, p.seated),
    pace: pose === "Pace",
    sitOn: !p.seated && p.depth !== p.gx + p.gy,
  };
}

export class ActorLayer {
  private actors = new Map<string, Actor>();
  private world: Container | null = null;
  private toCss: ToCss = (x, y) => ({ left: x, top: y });
  private hovered: string | null = null;
  private nametag: HTMLElement;
  private nametagName: HTMLElement;
  private nametagText: HTMLElement;
  private lastOffice: OfficeDto | null = null;
  private visitors: Visitor[] = [];
  /** 사장님이 오면 인터폰이 울린다 (소품 층이 건다) */
  onRing: ((now: number) => void) | null = null;
  /** 직원을 눌렀을 때 (직원 상세 팝업). 마운트하는 쪽이 건다 */
  onPick: ((teamId: string, agentId: string) => void) | null = null;

  /** `office` 는 손에 드는 물건(held_*)을 꺼내는 사무실 시트, `overlay` 는 이름표·Zzz·배지를 넣을 DOM 층. */
  constructor(
    private fp: Floorplan,
    private office: Spritesheet,
    private owls: Owls | null,
    private overlay: HTMLElement,
  ) {
    const doc = overlay.ownerDocument;
    this.nametag = doc.createElement("div");
    this.nametag.className = "ow-nametag";
    this.nametag.hidden = true;
    this.nametagName = doc.createElement("b");
    this.nametagText = doc.createElement("span");
    this.nametag.append(this.nametagName, doc.createElement("br"), this.nametagText);
    overlay.append(this.nametag);
  }

  /** 장면을 새로 지으면 옛 장면을 지우기 전에 불러 액터를 새 월드로 옮긴다. */
  attach(world: Container): void {
    this.world = world;
    for (const a of this.actors.values()) world.addChild(a.root);
    for (const v of this.visitors) {
      world.addChild(v.body.root);
      if (v.body.bubble && !v.body.bubble.isConnected)
        this.overlay.insertBefore(v.body.bubble, this.nametag);
    }
  }

  /**
   * 스냅샷 전체 교체. 같은 (team_id, agent_id) 는 스프라이트·DOM 을 그대로 쓰고, 목록에 없는 직원은 지운다. 그린 자리를 돌려준다.
   * 자리가 바뀐 직원은 큐에 걷기를 넣는다. `instant` 면 큐를 비우고 바로 제자리, cue 는 버린다
   * (사무실 전환 첫 스냅샷, 숨은 창에서 돌아옴). 조용한 재구성도 바로 제자리지만 백엔드가 남긴 출근 cue 는 재생한다 (PLAN 4.3).
   * `earlier` 는 두 sync 사이에 도착해 지나간 DTO 들의 cue 다. `office` 자신의 cue 앞에 이어 재생한다.
   */
  sync(
    office: OfficeDto | null,
    kind: Kind,
    now: number,
    opts: { instant?: boolean; earlier?: CueDto[] } = {},
  ): Placed[] {
    // 목록만 바뀌어도 같은 스냅샷으로 다시 불린다. 그때 cue 를 또 재생하지 않는다
    const fresh = office !== this.lastOffice;
    this.lastOffice = office;
    const instant = opts.instant || (fresh && !!office?.rebuilt);
    const cuesOf = new Map<string, CueDto[]>();
    const visits: CueDto[] = [];
    for (const c of fresh && !opts.instant
      ? [...(opts.earlier ?? []), ...(office?.cues ?? [])]
      : [])
      if ("agent" in c) {
        const key = empKey(c.teamId, c.agent);
        cuesOf.set(key, [...(cuesOf.get(key) ?? []), c]);
      } else if (c.kind === "BossVisit") visits.push(c);
      else playCue(c, undefined, now);
    const placed = office ? layoutActors(this.fp, kind, office) : [];
    const present = new Set(
      office?.teams.flatMap((t) => t.employees.map((e) => empKey(t.teamId, e.agentId))),
    );
    for (const [key, a] of this.actors)
      if (!present.has(key)) {
        a.root.destroy({ children: true });
        a.zz?.remove();
        a.badge?.remove();
        a.bubble?.remove();
        this.actors.delete(key);
        if (this.hovered === key) this.hover(null);
      }
    const shown = new Set<string>();
    const seqBase = this.fp.layouts[kind].placements.length;
    placed.forEach((p, i) => {
      const key = empKey(p.team.teamId, p.emp.agentId);
      const old = this.actors.get(key);
      const a = old ?? this.create(p.team.teamId, p.emp, key, now);
      shown.add(key);
      a.emp = p.emp;
      a.species = this.owls ? speciesOf(a.id, this.owls.meta.employeeSpecies) : "";
      a.outfit = `${p.emp.rank.toLowerCase()}-${themeOf(p.team.building)}`;
      a.role = p.emp.role === "Trainee" ? null : p.emp.role.toLowerCase();
      // Pixi 는 직원이 다른 자리로 옮겨져도 pointerout 을 안 내서 이름표가 새 자리를 따라간다
      if (a.spot !== p.spot && this.hovered === key) this.hover(null);
      a.spot = p.spot;
      a.seq = seqBase + i;
      const cues = cuesOf.get(key) ?? [];
      // 재출근도 문에서 걷지만, 탕비실 등에 보이던 퇴장 서브는 있던 자리에서 걷는다
      const entering =
        cues.some((c) => c.kind === "Enter") ||
        (cues.some((c) => c.kind === "Rehire") && !old?.root.visible);
      const target = targetOf(p);
      // cue 없이 처음 보는 직원은 놓친 cue 로 보고 제자리에 둔다 (PLAN 4.8)
      if (instant || (!old && !entering)) a.track.reset(target, now);
      if (entering || (old && !instant)) a.track.retarget(target, now, entering);
      for (const c of cues) {
        // 반려는 지나간 일이라 조용한 재구성에서는 다시 보이지 않는다
        if (c.kind === "Reject" && instant) continue;
        playCue(c, a.track, now);
        if (c.kind === "Reject") this.stamp(a);
      }
      this.setBadge(a);
      this.setBubble(a, now);
      if (!a.root.parent && this.world) this.world.addChild(a.root);
    });
    for (const a of this.actors.values())
      if (!shown.has(a.key)) {
        if (instant) a.track.reset(HIDDEN, now);
        else a.track.retarget(HIDDEN, now);
        for (const c of cuesOf.get(a.key) ?? []) playCue(c, a.track, now);
        a.zz?.remove();
        a.zz = null;
        a.badge?.remove();
        a.badge = null;
        a.bubble?.remove();
        a.bubble = null;
        a.spot = "";
        if (this.hovered === a.key) this.hover(null);
      }
    // 방문 중에 사무실을 옮기거나 그 팀이 퇴근하면 사장님도 없다
    for (const v of this.visitors.filter(
      (v) => instant || !office?.teams.some((t) => t.teamId === v.teamId && t.phase !== "Off"),
    ))
      this.dropVisitor(v);
    // 부장 큐가 새 자리로 정해진 뒤에 펄쩍을 얹는다
    if (office && !instant)
      for (const c of visits) if (c.kind === "BossVisit") this.visit(c, office, kind, now);
    this.update(now);
    this.placeOverlay();
    return placed;
  }

  update(now: number): void {
    let moved = false;
    for (const a of this.actors.values()) moved = this.draw(a, now) || moved;
    for (const v of this.visitors) {
      if (!v.leaving && now >= v.leaveAt) {
        v.body.track.retarget(HIDDEN, now);
        v.leaving = true;
      }
      moved = this.draw(v.body, now) || moved;
      if (v.leaving && !v.body.root.visible) this.dropVisitor(v);
      else if (v.body.bubble)
        v.body.bubble.hidden = !(now >= v.arriveAt && now < v.arriveAt + BUBBLE_MS);
    }
    for (const a of this.actors.values())
      if (a.bubble) a.bubble.hidden = !(Math.abs(now - a.bubbleAt) < BUBBLE_MS);
    if (moved) this.placeOverlay();
    this.fillNametag(now);
  }

  /** 한 몸을 지금 모습으로 그린다. 오버레이를 다시 놓아야 하면 true */
  private draw(a: Body, now: number): boolean {
    const s = a.track.update(now);
    if (!s.at) {
      if (a.root.visible && this.hovered === a.key) this.hover(null);
      a.root.visible = false;
      return false;
    }
    let moved = false;
    const p = gridToScreen(s.at.gx, s.at.gy, s.at.z);
    const x = roundEven(p.x);
    const y = roundEven(p.y);
    if (x !== a.x || y !== a.y || s.flip !== a.root.scale.x < 0) moved = true;
    a.x = x;
    a.y = y;
    a.root.position.set(x, y);
    // 기준점 x=8 이 가운데라 뒤집어도 발이 제자리다 (HANDOFF 6.4)
    a.root.scale.x = s.flip ? -1 : 1;
    a.root.zIndex = zIndexOf(s.depth, a.seq);
    a.root.visible = true;
    if (s.look.pose !== a.look.pose || s.look.dir !== a.look.dir) a.animStart = now;
    if (s.look.pose !== a.look.pose || s.look.dir !== a.look.dir || s.look.held !== a.look.held) {
      a.look = s.look;
      this.setHeld(a);
      this.setZz(a);
      moved = true;
    }
    this.placeStamp(a, now);
    if (!this.owls) return moved;
    const pose = this.owls.meta.poses[a.look.pose];
    const n = pose?.dirs[a.look.dir] ?? 1;
    let frame = 0;
    if (pose && pose.fps > 0) frame = Math.floor(((now - a.animStart) * pose.fps) / 1000) % n;
    else if (a.look.pose === "idle" && a.look.dir === "f" && n > 1) {
      // f 1번은 3~5초마다 150ms 눈 깜빡임 (HANDOFF 6.4)
      if (now >= a.nextBlink + BLINK_MS) a.nextBlink = now + 3000 + Math.random() * 2000;
      frame = now >= a.nextBlink ? 1 : 0;
    }
    this.setFrame(a, frame);
    return moved;
  }

  /** 1× 캔버스 좌표 → 오버레이 CSS px 변환을 받아 DOM 요소를 다시 놓는다 (HANDOFF 6.1). */
  placeOverlay(toCss?: ToCss): void {
    if (toCss) this.toCss = toCss;
    for (const a of [...this.actors.values(), ...this.visitors.map((v) => v.body)]) {
      const h = this.head(a);
      if (a.zz) this.put(a.zz, h.x + ZZ_OFFSET[0], h.y + ZZ_OFFSET[1]);
      if (a.badge) this.put(a.badge, h.x + BADGE_OFFSET[0], h.y + BADGE_OFFSET[1]);
      if (a.bubble) this.put(a.bubble, h.x, h.y);
    }
    const a = this.hovered ? this.actors.get(this.hovered) : undefined;
    if (a) {
      const h = this.head(a);
      this.put(this.nametag, h.x, h.y);
    }
  }

  /** `key` 는 empKey(team_id, agent_id). null 이면 이름표를 닫는다. */
  hover(key: string | null): void {
    const a = key ? this.actors.get(key) : undefined;
    this.hovered = a?.root.visible ? a.key : null;
    this.nametag.hidden = !this.hovered;
    if (!a || !this.hovered) return;
    const ranks = phrases.ranks as Record<string, string>;
    this.nametagName.textContent = `${a.emp.name} ${ranks[a.emp.rank.toLowerCase()] ?? ""}`;
    this.placeOverlay();
    this.fillNametag(Date.now());
  }

  /** 테스트·개발 페이지용: 지금 그린 직원의 루트 컨테이너. */
  rootOf(teamId: string, agentId: string): Container | undefined {
    return this.actors.get(empKey(teamId, agentId))?.root;
  }

  private fillNametag(now: number): void {
    const a = this.hovered ? this.actors.get(this.hovered) : undefined;
    if (!a) return;
    const act = a.emp.activity;
    // {elapsed} 는 백엔드가 둔 자리일 때만 채운다. 에이전트가 쓴 글 속 같은 글자는 그대로 둔다
    const text = fillElapsed(act.sentence, (now - Date.parse(act.since)) / 1000, act.elapsed);
    if (this.nametagText.textContent !== text) this.nametagText.textContent = text;
  }

  private create(teamId: string, emp: EmployeeDto, key: string, now: number): Actor {
    const id = emp.agentId;
    const b = this.body(id, key, now);
    b.root.eventMode = "static";
    b.root.cursor = "pointer";
    b.root.hitArea = new Rectangle(-8, -23, 16, 24);
    b.root.on("pointerover", () => this.hover(key));
    b.root.on("pointerout", () => {
      if (this.hovered === key) this.hover(null);
    });
    b.root.on("pointertap", () => this.onPick?.(teamId, id));
    const a: Actor = Object.assign(b, { emp, bubbleAt: 0 });
    this.actors.set(key, a);
    return a;
  }

  private body(id: string, key: string, now: number): Body {
    const root = new Container();
    const layers: Sprite[] = [];
    if (this.owls) {
      const [ax, ay] = this.owls.meta.anchor;
      for (let i = 0; i < 3; i++) {
        const s = new Sprite();
        s.anchor.set(0);
        s.pivot.set(ax, ay);
        layers.push(s);
        root.addChild(s);
      }
    } else root.addChild(this.fallbackBody());
    return {
      id,
      key,
      root,
      layers,
      held: null,
      heldName: undefined,
      species: "",
      outfit: "",
      role: null,
      look: IDLE,
      spot: "",
      x: Number.NaN,
      y: Number.NaN,
      seq: 0,
      track: new Track({
        fp: this.fp,
        seed: Number(fnv1a64(id) & 0xffffffffn),
        reduced: reducedMotion,
      }),
      animStart: now,
      nextBlink: now + 3000 + Math.random() * 2000,
      zz: null,
      badge: null,
      bubble: null,
      stamp: null,
      stampArmed: false,
      stampUntil: 0,
    };
  }

  /**
   * 사장님 방문 (S03): 문에서 부장 책상 앞까지 걸어와 5초 서 있다 나간다. 인터폰이 울리고 부장이 펄쩍 뛴다.
   * 사장님은 직원이 아니라 연출용 액터라 카드·이름표·눌림이 없다.
   */
  private visit(
    c: Extract<CueDto, { kind: "BossVisit" }>,
    office: OfficeDto,
    kind: Kind,
    now: number,
  ): void {
    const team = office.teams.find((t) => t.teamId === c.teamId);
    const lead = team?.employees[0];
    if (!team || !lead || team.phase === "Off") return;
    if (kind === "small" && team.deskSlot === null) return;
    const { spots, index } = seatSpots(this.fp, kind, team, lead, true);
    const seat = spots[Math.min(index, spots.length - 1)];
    if (!seat) return;
    // 같은 팀에 또 오면 앞 사장님은 없던 것으로
    for (const v of this.visitors.filter((v) => v.teamId === c.teamId)) this.dropVisitor(v);
    this.onRing?.(now);
    this.actors.get(empKey(c.teamId, lead.agentId))?.track.pose(JUMP, JUMP_MS, now);
    // 그 부장 좌석 앞(화면 아래쪽)에서 부장을 마주 본다
    const to = { gx: seat.gx, gy: seat.gy + VISIT_FRONT, z: 0 };
    const id = `boss-visit:${c.teamId}`;
    const b = this.body(id, id, now);
    if (this.owls) {
      b.species = "snowy";
      b.outfit = "boss";
    }
    const door = stand(this.fp, "door");
    const plain = { wander: false, pace: false, sitOn: false };
    b.track.reset({ at: door, look: IDLE, depth: door.gx + door.gy, ...plain }, now);
    // 책상 앞에서 부장을 마주 보면 화면에는 등이 보인다
    b.track.retarget(
      { at: to, look: { pose: "idle", dir: "b" }, depth: to.gx + to.gy, ...plain },
      now,
    );
    // 같은 depth 의 소품·직원 위에 그린다
    b.seq = this.fp.layouts[kind].placements.length + 1000;
    const arriveAt = now + (reducedMotion() ? 0 : walkMs(walkPath(this.fp, door, to)));
    const text = Array.from(c.text);
    b.bubble = this.overlay.ownerDocument.createElement("div");
    b.bubble.setAttribute("aria-hidden", "true");
    b.bubble.className = "ow-bubble";
    b.bubble.hidden = true;
    b.bubble.textContent =
      text.length > BOSS_TEXT_MAX ? `${text.slice(0, BOSS_TEXT_MAX).join("")}…` : c.text;
    this.overlay.insertBefore(b.bubble, this.nametag);
    if (this.world) this.world.addChild(b.root);
    this.visitors.push({
      teamId: c.teamId,
      body: b,
      arriveAt,
      leaveAt: arriveAt + VISIT_STAY_MS,
      leaving: false,
    });
  }

  private dropVisitor(v: Visitor): void {
    v.body.root.destroy({ children: true });
    v.body.bubble?.remove();
    this.visitors = this.visitors.filter((x) => x !== v);
  }

  /** 반려 도장: 결재판(손) 자리에 1초 */
  private stamp(a: Body): void {
    a.stamp ??= placeSprite(this.office, "stamp_reject/default", 0, 0);
    a.stampArmed = true;
    if (!a.stamp.parent) a.root.addChild(a.stamp);
  }

  private placeStamp(a: Body, now: number): void {
    if (!a.stamp) return;
    // 걷기 뒤에 긁기가 시작돼도 긁는 동안 1초 보인다
    const scratching = a.look.pose === "scratch";
    if (a.stampArmed && scratching) {
      a.stampArmed = false;
      a.stampUntil = now + STAMP_MS;
    }
    a.stamp.visible = scratching && now < a.stampUntil;
    if (!a.stamp.visible || !this.owls) return;
    // 몸이 좌우로 뒤집혀도 도장 글자는 뒤집지 않는다
    a.stamp.scale.x = a.root.scale.x;
    const { anchors, anchor } = this.owls.meta;
    const hand = (anchors[`${a.look.pose}_${a.look.dir}_0`] ?? anchors.carry_f_0).hand;
    a.stamp.position.set(hand[0] - anchor[0], hand[1] - anchor[1] - 4);
  }

  private fallbackBody(): Graphics {
    return new Graphics()
      .rect(-6, -18, 12, 18)
      .fill(0x8a6a4a)
      .rect(-4, -14, 3, 3)
      .rect(1, -14, 3, 3)
      .fill(0xffffff)
      .rect(-3, -13, 1, 1)
      .rect(2, -13, 1, 1)
      .fill(0x18181b);
  }

  private setFrame(a: Body, frame: number): void {
    if (!this.owls) return;
    const keys = owlFrameKeys(this.owls.textures, a.species, a.outfit, a.role, a.look, frame);
    keys.forEach((k, i) => {
      const s = a.layers[i];
      s.visible = k !== null;
      if (k) s.texture = this.owls!.textures[k];
    });
  }

  private setHeld(a: Body): void {
    const name = this.owls ? a.look.held : undefined;
    if (name === a.heldName) return;
    a.held?.destroy();
    a.held = null;
    a.heldName = name;
    if (!name || !this.owls) return;
    const anchor = this.owls.meta.anchors[`${a.look.pose}_${a.look.dir}_0`];
    if (!anchor) return;
    const [ax, ay] = this.owls.meta.anchor;
    a.held = placeSprite(
      this.office,
      `held_${name}/default`,
      anchor.hand[0] - ax,
      anchor.hand[1] - ay,
    );
    a.root.addChild(a.held);
  }

  private setZz(a: Body): void {
    const want = a.look.pose === "sleep";
    if (want && !a.zz) {
      a.zz = this.overlay.ownerDocument.createElement("div");
      a.zz.className = "ow-zz";
      a.zz.textContent = "Zzz";
      this.overlay.insertBefore(a.zz, this.nametag);
    } else if (!want && a.zz) {
      a.zz.remove();
      a.zz = null;
    }
  }

  private setBadge(a: Actor): void {
    const n = a.emp.activity.extra;
    if (n > 0 && !a.badge) {
      a.badge = this.overlay.ownerDocument.createElement("div");
      a.badge.className = "ow-parallel";
      this.overlay.insertBefore(a.badge, this.nametag);
    } else if (n <= 0 && a.badge) {
      a.badge.remove();
      a.badge = null;
    }
    if (a.badge) a.badge.textContent = `+${n}`;
  }

  private setBubble(a: Actor, now: number): void {
    const b = a.emp.bubble;
    a.bubbleAt = b ? Date.parse(b.at) : Number.NaN;
    // 옛 말풍선(재구성·다시 불러오기 뒤)은 만들지도 않는다. at 이 4초 넘게 앞서도(WSL 시계가 앞섬) 안 그린다
    const text =
      b && Math.abs(now - a.bubbleAt) < BUBBLE_MS ? bubbleText(a.id, a.emp.rank, b) : null;
    if (text === null) {
      a.bubble?.remove();
      a.bubble = null;
      return;
    }
    if (!a.bubble) {
      a.bubble = this.overlay.ownerDocument.createElement("div");
      a.bubble.setAttribute("aria-hidden", "true");
      // 이름표가 말풍선과 겹치면 이름표가 위 (DOM 순서)
      this.overlay.insertBefore(a.bubble, this.nametag);
    }
    const act = a.emp.activity;
    a.bubble.className = "ow-bubble";
    a.bubble.classList.toggle("is-estimate", act.confidence === "Inferred");
    a.bubble.classList.toggle("is-incident", b?.say.kind === "Incident");
    a.bubble.classList.toggle("is-ask", act.room === "Door");
    a.bubble.textContent = text;
  }

  private head(a: Body): { x: number; y: number } {
    const meta = this.owls?.meta;
    const anchor = meta?.anchors[`${a.look.pose}_${a.look.dir}_0`];
    if (!meta || !anchor) return { x: a.x, y: a.y - 18 };
    return {
      x: a.x + (anchor.head[0] - meta.anchor[0]) * a.root.scale.x,
      y: a.y + anchor.head[1] - meta.anchor[1],
    };
  }

  private put(el: HTMLElement, x: number, y: number): void {
    const p = this.toCss(x, y);
    el.style.left = `${p.left}px`;
    el.style.top = `${p.top}px`;
  }
}
