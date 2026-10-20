// 직원 액터: 스냅샷의 직원을 방·좌석에 세우고 세 겹 올빼미로 그린다 (HANDOFF 4.4, 5, 6.4).
import { Container, Graphics, Rectangle, Sprite, type Spritesheet, type Texture } from "pixi.js";
import phrases from "../../design/handoff/phrases.ko.json";
import type { EmployeeDto, OfficeDto, Pose, Room, TeamDto } from "../types/ipc.ts";
import { gridToScreen, themeOf, zIndexOf, type Floorplan, type Kind } from "./floorplan.ts";
import { placeSprite } from "./sprites.ts";

const BLINK_MS = 150;
// ponytail: "+n" 배지(ow-parallel)는 Activity.extra 가 DTO 에 아직 없어 뺐다, 백엔드가 보내면 head 기준점 +14,+6 에 붙인다
const ZZ_OFFSET: [number, number] = [6, -10];

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
  /** 목록에서 먼저 볼 자리. 장애 상황실 0번은 부장 몫이다 */
  from?: number;
}

const IDLE: Look = { pose: "idle", dir: "f" };
const SIT: Look = { pose: "sit", dir: "f" };

const SEAT_LOOK: Record<Pose, Look> = {
  Sit: SIT,
  Type: { pose: "type", dir: "f" },
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
  Pantry: { at: ["pantry_console", "pantry"], look: SIT },
  // 백엔드는 팀원을 장애 상황실로 보내지 않지만 표대로 둔다
  Incident: { at: ["incident"], look: IDLE, from: 1 },
  // 부장 아닌 직원이 팀장실, 부장이 개발실에 있을 때
  TeamLead: { at: ["boss_pace"], look: IDLE },
  Dev: { at: ["dev_center"], look: IDLE },
};

/** 방·자세 → 자리와 sprite 자세. null 은 그리지 않는다 (외근). */
export function placeRule(room: Room, pose: Pose, boss: boolean): Rule | null {
  if (room === "Outside") return null;
  if (room === (boss ? "TeamLead" : "Dev")) return { at: "seat", look: SEAT_LOOK[pose] };
  if (room === "Incident" && boss) return { at: ["incident"], look: { pose: "panic", dir: "f" } };
  if (room === "Pantry" && pose === "Sleep")
    return { at: ["pantry_sofa", "pantry_massage"], look: SEAT_LOOK.Sleep };
  const rule = ROOM_RULE[room];
  if (!rule) return null;
  if (pose === "Squat" || pose === "Sleep") return { ...rule, look: SEAT_LOOK[pose] };
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
}

export interface Placed {
  emp: EmployeeDto;
  team: TeamDto;
  spot: string;
  x: number;
  y: number;
  depth: number;
  look: Look;
}

/** 참조 렌더(Python round)와 같은 은행가 반올림. 의자·소파 자리 몇 곳이 정확히 .5 에 떨어진다. */
function roundEven(v: number): number {
  const f = Math.floor(v);
  const d = v - f;
  if (d !== 0.5) return Math.round(v);
  return f % 2 === 0 ? f : f + 1;
}

function standSpots(fp: Floorplan, kind: Kind, names: string[]): Spot[] {
  const placements = fp.layouts[kind].placements;
  return names.flatMap((name) =>
    (fp.stands[name] ?? []).map(([gx, gy, z = 0], i) => {
      const on = SIT_ON[name] && placements.find((p) => p.id === SIT_ON[name]);
      return { id: `${name}_${i}`, gx, gy, z, depth: on ? on.depth : gx + gy };
    }),
  );
}

/** 좌석 목록과 그중 이 직원의 번호. 번호가 없으면 마지막 자리에 겹친다. */
function seatSpots(
  fp: Floorplan,
  kind: Kind,
  team: TeamDto,
  emp: EmployeeDto,
  boss: boolean,
): { spots: Spot[]; index: number } | null {
  const s = fp.layouts[kind].seats;
  const toSpot = (seat: { id: string; gx: number; gy: number; depth: number }): Spot => ({
    ...seat,
    z: 0,
  });
  if (!boss) {
    const spots = [...s.dev, ...s.temp].map(toSpot);
    // ponytail: 백엔드는 10명째 뒤 직원에게 좌석을 안 준다(카드만). 문서(3.4)대로 마지막 임시 책상에 겹친다, 만석 쪽지는 소품 티켓(T23)
    return { spots, index: emp.seat ?? spots.length - 1 };
  }
  if (kind === "large") return { spots: s.boss.map(toSpot), index: 0 };
  // 팀장석 0~3, 간이 4~5, 그 밖은 카드만 (PLAN 4.7.9)
  if (team.deskSlot === null) return null;
  return { spots: [...s.boss, ...s.spare].map(toSpot), index: team.deskSlot };
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
      const rule = placeRule(emp.activity.room, emp.activity.pose, boss);
      if (!rule) return;
      let spots: Spot[];
      let start: number;
      if (rule.at === "seat") {
        const seat = seatSpots(fp, kind, team, emp, boss);
        if (!seat) return;
        spots = seat.spots;
        start = seat.index;
      } else {
        spots = standSpots(fp, kind, rule.at);
        start = rule.from ?? 0;
      }
      if (spots.length === 0) return;
      start = Math.min(Math.max(0, start), spots.length - 1);
      let k = start;
      while (k < spots.length && used.has(spots[k].id)) k++;
      // ponytail: 자리가 모자라면 마지막 자리에 겹친다, 겹침 연출(만석 쪽지 등)은 T23·T29
      const spot = spots[Math.min(k, spots.length - 1)];
      used.add(spot.id);
      const p = gridToScreen(spot.gx, spot.gy, spot.z);
      out.push({
        emp,
        team,
        spot: spot.id,
        x: roundEven(p.x),
        y: roundEven(p.y),
        depth: spot.depth,
        look: rule.look,
      });
    });
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
  x: number;
  y: number;
  flip: boolean;
  animStart: number;
  nextBlink: number;
  emp: EmployeeDto;
  zz: HTMLElement | null;
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

  /**
   * `office` 는 손에 드는 물건(held_*)을 꺼내는 사무실 시트, `overlay` 는 이름표·Zzz 를 넣을 DOM 층.
   * `random` 은 눈 깜빡임 간격용 (테스트에서 고정).
   */
  constructor(
    private fp: Floorplan,
    private office: Spritesheet,
    private owls: Owls | null,
    private overlay: HTMLElement,
    private random: () => number = Math.random,
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

  /** 스냅샷 전체 교체. 같은 agent_id 는 스프라이트·DOM 을 그대로 쓰고, 목록에 없는 직원은 지운다. */
  sync(office: OfficeDto | null, kind: Kind, now: number): void {
    const placed = office ? layoutActors(this.fp, kind, office) : [];
    const present = new Set(office?.teams.flatMap((t) => t.employees.map((e) => e.agentId)));
    for (const [id, a] of this.actors)
      if (!present.has(id)) {
        a.root.destroy({ children: true });
        a.zz?.remove();
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
      // ponytail: 방이 바뀌면 바로 옮긴다, 걷기·큐는 T22
      // 왼쪽으로 옮겨 갔으면 왼쪽을 본다. 처음 놓일 때(x NaN)는 오른쪽
      if (p.x < a.x) a.flip = true;
      else if (p.x > a.x) a.flip = false;
      a.x = p.x;
      a.y = p.y;
      if (a.look.pose !== p.look.pose || a.look.dir !== p.look.dir) a.animStart = now;
      a.look = p.look;
      a.root.position.set(p.x, p.y);
      a.root.scale.x = a.flip ? -1 : 1;
      a.root.zIndex = zIndexOf(p.depth, seqBase + i);
      a.root.visible = true;
      this.setHeld(a);
      this.setFrame(a, 0);
      this.setZz(a);
      if (!a.root.parent && this.world) this.world.addChild(a.root);
    });
    for (const a of this.actors.values())
      if (!shown.has(a.id)) {
        a.root.visible = false;
        a.zz?.remove();
        a.zz = null;
        if (this.hovered === a.id) this.hover(null);
      }
    this.placeOverlay();
    this.update(now);
  }

  /** 틱마다: 자세 애니메이션, 눈 깜빡임, 이름표의 경과 시간. */
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
          if (now >= a.nextBlink + BLINK_MS) a.nextBlink = now + 3000 + this.random() * 2000;
          frame = now >= a.nextBlink ? 1 : 0;
        }
        this.setFrame(a, frame);
      }
    if (this.hovered) {
      const a = this.actors.get(this.hovered);
      if (a) {
        const text = fillElapsed(a.emp.activity.sentence, a.emp.activity.since, now);
        if (this.nametagText.textContent !== text) this.nametagText.textContent = text;
      }
    }
  }

  /** 1× 캔버스 좌표 → 오버레이 CSS px 변환을 받아 DOM 요소를 다시 놓는다 (HANDOFF 6.1). */
  placeOverlay(toCss?: ToCss): void {
    if (toCss) this.toCss = toCss;
    for (const a of this.actors.values()) {
      if (!a.zz) continue;
      const h = this.head(a);
      this.put(a.zz, h.x + (a.flip ? -ZZ_OFFSET[0] : ZZ_OFFSET[0]), h.y + ZZ_OFFSET[1]);
    }
    const a = this.hovered ? this.actors.get(this.hovered) : undefined;
    if (a) {
      const h = this.head(a);
      this.put(this.nametag, h.x, h.y);
    }
  }

  /** 마우스를 올린 직원의 이름표 (HANDOFF 4.4). null 이면 숨긴다. */
  hover(agentId: string | null): void {
    const a = agentId ? this.actors.get(agentId) : undefined;
    this.hovered = a?.root.visible ? a.id : null;
    this.nametag.hidden = !this.hovered;
    if (!a || !this.hovered) return;
    const ranks = phrases.ranks as Record<string, string>;
    this.nametagName.textContent = `${a.emp.name} ${ranks[a.emp.rank.toLowerCase()] ?? ""}`;
    this.nametagText.textContent = "";
    this.placeOverlay();
    this.update(Date.now());
  }

  /** 테스트·개발 페이지용: 지금 그린 직원의 루트 컨테이너. */
  rootOf(agentId: string): Container | undefined {
    return this.actors.get(agentId)?.root;
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
      x: Number.NaN,
      y: Number.NaN,
      flip: false,
      animStart: now,
      nextBlink: now + 3000 + this.random() * 2000,
      emp,
      zz: null,
    };
    this.actors.set(id, a);
    return a;
  }

  /** 아틀라스가 없을 때: 단색 사각 + 눈 (PLAN 4.9). 발 기준점이 (0,0). */
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
    // 컨테이너를 뒤집으면 손 위치와 물건이 같이 뒤집혀 참조 렌더의 flip 과 같아진다
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

  /** `head` 기준점의 1× 캔버스 좌표. 아틀라스가 없으면 대체 몸체 머리. */
  private head(a: Actor): { x: number; y: number } {
    const meta = this.owls?.meta;
    const anchor = meta?.anchors[`${a.look.pose}_${a.look.dir}_0`];
    if (!meta || !anchor) return { x: a.x, y: a.y - 18 };
    const dx = anchor.head[0] - meta.anchor[0];
    return { x: a.x + (a.flip ? -dx : dx), y: a.y + anchor.head[1] - meta.anchor[1] };
  }

  private put(el: HTMLElement, x: number, y: number): void {
    const p = this.toCss(x, y);
    el.style.left = `${p.left}px`;
    el.style.top = `${p.top}px`;
  }
}
