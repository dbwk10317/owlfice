// 직원 액터: 스냅샷의 직원을 방·좌석에 세우고 세 겹 올빼미로 그린다 (HANDOFF 4.4, 5, 6.4).
import { Container, Graphics, Rectangle, Sprite, type Spritesheet, type Texture } from "pixi.js";
import phrases from "../../design/handoff/phrases.ko.json";
import type { Bubble, EmployeeDto, OfficeDto, Pose, Room, TeamDto } from "../types/ipc.ts";
import { gridToScreen, themeOf, zIndexOf, type Floorplan, type Kind } from "./floorplan.ts";
import { placeSprite } from "./sprites.ts";

const BLINK_MS = 150;
const ZZ_OFFSET: [number, number] = [6, -10];
const BADGE_OFFSET: [number, number] = [14, 6];
const BUBBLE_MS = 4000;

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
  // 결재 대기. 왕복 걷기는 T22 라 그때까지 boss_pace 에 서 있는다
  // ponytail: 왕복 없이 첫 자리에 idle, 걷기 티켓(T22)에서 두 자리를 오간다
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
  look?: Look;
}

export interface Placed {
  emp: EmployeeDto;
  team: TeamDto;
  spot: string;
  /** 자리가 모자라 마지막 자리에 겹쳤다 */
  overflow: boolean;
  x: number;
  y: number;
  depth: number;
  look: Look;
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
  const toSpot = (seat: { id: string; gx: number; gy: number; depth: number }): Spot => ({
    ...seat,
    z: 0,
  });
  if (!boss) {
    const spots = [...s.dev, ...s.temp].map(toSpot);
    // ponytail: 백엔드는 10명째 뒤 직원에게 좌석을 안 준다(카드만). 문서(3.4)대로 마지막 임시 책상에 겹치고 만석 쪽지를 붙인다
    return { spots, index: emp.seat ?? spots.length - 1 };
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
      const overflow = k >= spots.length;
      // ponytail: 자리가 모자라면 마지막 자리에 겹친다, 겹침 연출은 T29
      const spot = spots[Math.min(k, spots.length - 1)];
      used.add(spot.id);
      const p = gridToScreen(spot.gx, spot.gy, spot.z);
      out.push({
        emp,
        team,
        spot: spot.id,
        overflow,
        x: roundEven(p.x),
        y: roundEven(p.y),
        depth: spot.depth,
        look: spot.look ?? rule.look,
      });
    });
  }
  return out;
}

/**
 * 평면도가 숨겨 둔 소품 중 지금 보여야 할 것: 임시·간이 책상과 쪽지 (HANDOFF 6.5).
 * 만석 쪽지는 임시 책상 쪽지와 같은 자리에 놓고 그것을 대신한다.
 */
export function seatProps(fp: Floorplan, kind: Kind, placed: Placed[]): Set<string> {
  const s = fp.layouts[kind].seats;
  const out = new Set<string>();
  for (const p of placed) {
    const seat = [...s.temp, ...s.spare].find((x) => x.id === p.spot);
    if (!seat) continue;
    out.add(seat.desk);
    if (s.temp.includes(seat)) out.add(p.overflow ? "note_full" : "note_temp");
  }
  if (out.has("note_full")) out.delete("note_temp");
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

// ---------- 문장 ----------

const fill = (tpl: string, vars: Record<string, string | number>) =>
  tpl.replace(/\{(\w+)\}/g, (_, k: string) => String(vars[k] ?? ""));

function durationText(sec: number): string {
  const u = phrases.units;
  if (!(sec >= 60)) return fill(u.sec, { n: Math.max(0, Math.floor(sec) || 0) });
  const min = Math.floor(sec / 60);
  if (min >= 60) return fill(u.hourMin, { h: Math.floor(min / 60), m: min % 60 });
  const rest = Math.floor(sec) % 60;
  return rest ? `${fill(u.min, { n: min })} ${fill(u.sec, { n: rest })}` : fill(u.min, { n: min });
}

/** 문장의 `{elapsed}` 를 since 부터 지금까지로 채운다. 백틱 코드 조각 안은 그대로 둔다 (PLAN 4.8). */
export function fillElapsed(sentence: string, sinceIso: string, now: number): string {
  if (!sentence.includes("{elapsed}")) return sentence;
  const text = durationText((now - Date.parse(sinceIso)) / 1000);
  return sentence
    .split("`")
    .map((part, i) => (i % 2 === 0 ? part.split("{elapsed}").join(text) : part))
    .join("`");
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

interface Actor {
  id: string;
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
  animStart: number;
  nextBlink: number;
  emp: EmployeeDto;
  zz: HTMLElement | null;
  badge: HTMLElement | null;
  bubble: HTMLElement | null;
  /** 말풍선 사건 시각(ms). 4초가 지나면 update 가 숨긴다 */
  bubbleAt: number;
}

type ToCss = (x: number, y: number) => { left: number; top: number };

export class ActorLayer {
  private actors = new Map<string, Actor>();
  private world: Container | null = null;
  private toCss: ToCss = (x, y) => ({ left: x, top: y });
  private hovered: string | null = null;
  private nametag: HTMLElement;
  private nametagName: HTMLElement;
  private nametagText: HTMLElement;

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
  }

  /** 스냅샷 전체 교체. 같은 agent_id 는 스프라이트·DOM 을 그대로 쓰고, 목록에 없는 직원은 지운다. 그린 자리를 돌려준다. */
  sync(office: OfficeDto | null, kind: Kind, now: number): Placed[] {
    const placed = office ? layoutActors(this.fp, kind, office) : [];
    const present = new Set(office?.teams.flatMap((t) => t.employees.map((e) => e.agentId)));
    for (const [id, a] of this.actors)
      if (!present.has(id)) {
        a.root.destroy({ children: true });
        a.zz?.remove();
        a.badge?.remove();
        a.bubble?.remove();
        this.actors.delete(id);
        if (this.hovered === id) this.hover(null);
      }
    const shown = new Set<string>();
    const seqBase = this.fp.layouts[kind].placements.length;
    placed.forEach((p, i) => {
      const a = this.actors.get(p.emp.agentId) ?? this.create(p.emp, now);
      shown.add(a.id);
      a.emp = p.emp;
      a.species = this.owls ? speciesOf(a.id, this.owls.meta.employeeSpecies) : "";
      a.outfit = `${p.emp.rank.toLowerCase()}-${themeOf(p.team.building)}`;
      a.role = p.emp.role === "Trainee" ? null : p.emp.role.toLowerCase();
      // Pixi 는 직원이 다른 자리로 옮겨져도 pointerout 을 안 내서 이름표가 새 자리를 따라간다
      if (a.spot !== p.spot && this.hovered === a.id) this.hover(null);
      // ponytail: 방이 바뀌면 바로 옮긴다, 걷기·큐·좌우 반전은 T22
      a.spot = p.spot;
      a.x = p.x;
      a.y = p.y;
      if (a.look.pose !== p.look.pose || a.look.dir !== p.look.dir) a.animStart = now;
      a.look = p.look;
      a.root.position.set(p.x, p.y);
      a.root.zIndex = zIndexOf(p.depth, seqBase + i);
      a.root.visible = true;
      this.setHeld(a);
      this.setFrame(a, 0);
      this.setZz(a);
      this.setBadge(a);
      this.setBubble(a, now);
      if (!a.root.parent && this.world) this.world.addChild(a.root);
    });
    for (const a of this.actors.values())
      if (!shown.has(a.id)) {
        a.root.visible = false;
        a.zz?.remove();
        a.zz = null;
        a.badge?.remove();
        a.badge = null;
        a.bubble?.remove();
        a.bubble = null;
        a.spot = "";
        if (this.hovered === a.id) this.hover(null);
      }
    this.placeOverlay();
    this.update(now);
    return placed;
  }

  update(now: number): void {
    if (this.owls)
      for (const a of this.actors.values()) {
        if (!a.root.visible) continue;
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
      }
    for (const a of this.actors.values())
      if (a.bubble) a.bubble.hidden = !(Math.abs(now - a.bubbleAt) < BUBBLE_MS);
    this.fillNametag(now);
  }

  /** 1× 캔버스 좌표 → 오버레이 CSS px 변환을 받아 DOM 요소를 다시 놓는다 (HANDOFF 6.1). */
  placeOverlay(toCss?: ToCss): void {
    if (toCss) this.toCss = toCss;
    for (const a of this.actors.values()) {
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

  hover(agentId: string | null): void {
    const a = agentId ? this.actors.get(agentId) : undefined;
    this.hovered = a?.root.visible ? a.id : null;
    this.nametag.hidden = !this.hovered;
    if (!a || !this.hovered) return;
    const ranks = phrases.ranks as Record<string, string>;
    this.nametagName.textContent = `${a.emp.name} ${ranks[a.emp.rank.toLowerCase()] ?? ""}`;
    this.placeOverlay();
    this.fillNametag(Date.now());
  }

  /** 테스트·개발 페이지용: 지금 그린 직원의 루트 컨테이너. */
  rootOf(agentId: string): Container | undefined {
    return this.actors.get(agentId)?.root;
  }

  private fillNametag(now: number): void {
    const a = this.hovered ? this.actors.get(this.hovered) : undefined;
    if (!a) return;
    const act = a.emp.activity;
    // {elapsed} 는 백엔드가 둔 자리일 때만 채운다. 에이전트가 쓴 글 속 같은 글자는 그대로 둔다
    const text = act.elapsed ? fillElapsed(act.sentence, act.since, now) : act.sentence;
    if (this.nametagText.textContent !== text) this.nametagText.textContent = text;
  }

  private create(emp: EmployeeDto, now: number): Actor {
    const id = emp.agentId;
    const root = new Container();
    root.eventMode = "static";
    root.cursor = "pointer";
    root.hitArea = new Rectangle(-8, -23, 16, 24);
    root.on("pointerover", () => this.hover(id));
    root.on("pointerout", () => {
      if (this.hovered === id) this.hover(null);
    });
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
    const a: Actor = {
      id,
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
      animStart: now,
      nextBlink: now + 3000 + Math.random() * 2000,
      emp,
      zz: null,
      badge: null,
      bubble: null,
      bubbleAt: 0,
    };
    this.actors.set(id, a);
    return a;
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

  private setFrame(a: Actor, frame: number): void {
    if (!this.owls) return;
    const keys = owlFrameKeys(this.owls.textures, a.species, a.outfit, a.role, a.look, frame);
    keys.forEach((k, i) => {
      const s = a.layers[i];
      s.visible = k !== null;
      if (k) s.texture = this.owls!.textures[k];
    });
  }

  private setHeld(a: Actor): void {
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

  private setZz(a: Actor): void {
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

  private head(a: Actor): { x: number; y: number } {
    const meta = this.owls?.meta;
    const anchor = meta?.anchors[`${a.look.pose}_${a.look.dir}_0`];
    if (!meta || !anchor) return { x: a.x, y: a.y - 18 };
    return { x: a.x + anchor.head[0] - meta.anchor[0], y: a.y + anchor.head[1] - meta.anchor[1] };
  }

  private put(el: HTMLElement, x: number, y: number): void {
    const p = this.toCss(x, y);
    el.style.left = `${p.left}px`;
    el.style.top = `${p.top}px`;
  }
}
