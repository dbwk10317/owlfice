// 소품 상태: 신호등·화분·전광판·휴지통·칸반·피자·창밖·TV·미팅실·리뷰실·장애 상황실·외근 팻말 (HANDOFF 5, 6.5, 6.6, PLAN 3.4, 4.7.9).
// 값 → 상태 이름은 순수 함수, PropsLayer 는 바뀐 소품만 다시 그린다. 움직임은 cue 몫이다.
import { Graphics, type Sprite, type Spritesheet } from "pixi.js";
import type { CueDto, OfficeDto, TeamDto, TodoItem } from "../types/ipc.ts";
import { reducedMotion } from "../loading.ts";
import { empKey } from "../ticker.ts";
import {
  gridToScreen,
  hexToRgb,
  zIndexOf,
  type Floorplan,
  type Kind,
  type Seat,
} from "./floorplan.ts";
import { meetingCall, seated } from "./actors.ts";
import {
  DOOR_OPEN_MS,
  FIREWORKS_MS,
  LADDER_MS,
  REPOT_MS,
  RETURN_MS,
  RING_MS,
  STAMP_RESULT_MS,
  TRUCK_MS,
} from "./anim-queue.ts";
import type { Scene } from "./renderer.ts";
import { placeSprite } from "./sprites.ts";

type PropsTarget = Pick<Scene, "wall" | "world" | "driven">;

/** 평면도에서 상태나 보임을 코드가 바꾸는 소품. 숨겨진 채 시작하는 것(ramen, 중소기업 화분)도 장면에 만든다 */
export const DRIVEN_PROP =
  /^(ci_light|window|trash|safety_board|ramen|plant_boss_\d|interphone|mug_\d|duck_\d|boss_mon_\d_\d|rack_\d|terminal|game_tv|fan|test_machine|test_board|job_ticket|door|meeting_screen|review_screen|whiteboard|beacon|incident_table)$/;

/** 부장 자세로 정해지는 책상 소품 (HANDOFF 5절): 응답 대기 김, 러버덕 집어 듦, 결재 대기 화면보호기 */
export interface BossFx {
  steam: boolean;
  duck: boolean;
  saver: boolean;
}

export function bossFx(team: TeamDto | undefined): BossFx {
  const a = team?.phase === "Off" ? undefined : team?.employees?.[0]?.activity;
  // 앉는 자세는 팀장실에 있을 때만 책상 앞이다. 서성임은 방과 상관없이 결재 대기다
  const seated = a?.room === "TeamLead";
  return {
    steam: seated && a.pose === "Think",
    duck: seated && a.pose === "Duck",
    saver: a?.pose === "Pace",
  };
}

/** 직원이 어느 방에 있나로 정해지는 빌드실·테스트실·출고장 소품 (HANDOFF 5절) */
export interface LabFx {
  /** 빌드실에 직원이 있다: 랙 LED 반복, 단말기 켜짐 */
  shell: boolean;
  /** 빌드실 직원이 쪼그려 앉았다 (명령이 평균 2배 초과): 선풍기 */
  slow: boolean;
  /** 테스트실에 직원이 있다: 테스트 기계 가동 */
  test: boolean;
  /** 출고장에 있는 직원 수: 포장된 상자 */
  boxes: number;
}

// 빌드실·테스트실·출고장은 사무실이 하나뿐인 공용 방이라 어느 팀이든 일하면 켠다 (팀별로 가르는 bossFx 와 다른 이유)
// 백엔드 Room 하나가 셸 명령과 빌드를 같이 담아(DTO 로 가를 수 없다) 빌드 중에도 랙이 깜빡인다
export function labFx(office: OfficeDto | null): LabFx {
  const fx: LabFx = { shell: false, slow: false, test: false, boxes: 0 };
  for (const team of office?.teams ?? []) {
    if (team.phase === "Off") continue;
    for (const { activity: a } of team.employees) {
      if (a.room === "Build") {
        fx.shell = true;
        fx.slow ||= a.pose === "Squat";
      } else if (a.room === "Test") fx.test = true;
      else if (a.room === "Shipping") fx.boxes++;
    }
  }
  return fx;
}

/** 공용 방 소품 (HANDOFF 5절 미팅·스킬·리뷰) */
interface RoomFx {
  /** MCP 미팅 중인 거래처 상호. 없으면 null */
  vendor: string | null;
  /** 스킬 매뉴얼을 펼친 직원이 있다 */
  skill: boolean;
  /** 리뷰실에 직원이 있으면 빨간펜 수, 없으면 null */
  pens: number | null;
}

// 미팅실·리뷰실도 공용 방이라 어느 팀이든 일하면 켠다 (labFx 와 같은 규칙)
// ponytail: 빨간펜 수는 DTO 필드가 없어 백엔드 문장("빨간펜 n개")에서 읽는다, 필드가 생기면 그것으로
function roomFx(office: OfficeDto | null): RoomFx {
  const fx: RoomFx = { vendor: null, skill: false, pens: null };
  for (const team of office?.teams ?? []) {
    if (team.phase === "Off") continue;
    for (const { activity: a } of team.employees) {
      const call = meetingCall(a);
      if (call?.kind === "mcp") fx.vendor ??= call.server;
      else if (call) fx.skill = true;
      else if (a.room === "Review") {
        const n = Number(/빨간펜 (\d+)개/.exec(a.sentence)?.[1] ?? 0);
        fx.pens = Math.max(fx.pens ?? 0, n);
      }
    }
  }
  return fx;
}

export type MagnetColor =
  | "red"
  | "blue"
  | "green"
  | "yellow"
  | "gray"
  | "teal"
  | "violet"
  | "coral";

// 대기업은 직원 순번, 중소기업은 팀장석(deskSlot) 번호로 색을 고른다. 팀 순서가 바뀌어도 색이 같다 (HANDOFF 6.6)
const MAGNET_BY_EMPLOYEE: MagnetColor[] = [
  "red",
  "blue",
  "green",
  "yellow",
  "gray",
  "teal",
  "violet",
  "coral",
];
const MAGNET_BY_TEAM: MagnetColor[] = ["teal", "violet", "coral", "yellow", "blue", "gray"];
/** 근태판 칸 번호 (props.attendance.columns) */
const COL_IN = 0;
const COL_OUT = 1;
const COL_OFF = 3;

export interface Magnet {
  col: number;
  line: number;
  color: MagnetColor;
}

/**
 * 직원마다 자석 하나: 퇴근한 팀·퇴장한 서브는 퇴근, 외근은 외근, 나머지는 출근. 한 칸에 슬롯 수까지만 (넘치면 뒤 직원은 자석이 없다).
 * 중소기업은 모든 팀을 한 판에 합친다 (labFx 와 같은 규칙).
 * ponytail: 외출 칸은 DTO 로 가릴 수 있는 상태가 없어 비워 둔다, 외출을 뜻하는 상태가 생기면 채운다
 */
export function magnets(office: OfficeDto | null, kind: Kind, slots: number): Magnet[] {
  const used = [0, 0, 0, 0];
  const out: Magnet[] = [];
  for (const team of office?.teams ?? [])
    team.employees.forEach((emp, i) => {
      const col =
        team.phase === "Off" || emp.retiredAt !== null
          ? COL_OFF
          : emp.activity.room === "Outside"
            ? COL_OUT
            : COL_IN;
      const line = used[col]++;
      if (line >= slots) return;
      const color =
        kind === "large"
          ? MAGNET_BY_EMPLOYEE[i % MAGNET_BY_EMPLOYEE.length]
          : MAGNET_BY_TEAM[Math.min(team.deskSlot ?? MAGNET_BY_TEAM.length - 1, 5)];
      out.push({ col, line, color });
    });
  return out;
}

/**
 * 콘솔 게임기를 켜나. 탕비실에 있는 첫 직원이 콘솔 자리를 차지한다 (졸음은 소파, 서성임은 팀장실로 가서 뺀다).
 * actors.ts layoutActors 가 콘솔에 앉히는 경우와 같다
 */
export function gameOn(office: OfficeDto | null, kind: Kind): boolean {
  return (office?.teams ?? []).some(
    (t) =>
      t.phase !== "Off" &&
      t.employees.some(
        (e, i) =>
          e.activity.room === "Pantry" &&
          e.activity.pose !== "Sleep" &&
          e.activity.pose !== "Pace" &&
          !(i === 0 && kind === "small" && t.deskSlot === null),
      ),
  );
}

type BossSeat = Seat & { mug?: string; duck?: string };

const MUG_FPS = 3;
const SAVER_FPS = 2;
const RING_FPS = 8;
const RACK_FPS = 3;
const FAN_FPS = 10;
const BEACON_FPS = 6;
const GAME_FPS = 3;
const MACHINE_FPS = 6;
const TRUCK_FPS = 8;
const FIREWORKS_FPS = 6;
/** 2프레임 반복 소품의 지금 프레임 번호 */
const blink = (nowMs: number, fps: number) => Math.floor((nowMs * fps) / 1000) % 2;

/** 컨텍스트 사용률 0~1 → 화분 단계 (office.json plant 설명). 1 을 넘으면 시든다. null 이면 undefined = 평면도 기본 */
export function plantState(v: number | null): string | undefined {
  if (v === null || Number.isNaN(v)) return undefined;
  if (v > 1) return "wilted";
  for (const [limit, s] of [
    [0.2, "s0"],
    [0.4, "s1"],
    [0.6, "s2"],
    [0.8, "s3"],
  ] as const)
    if (v <= limit) return s;
  return "s4";
}

export const trashState = (n: number): string => (n <= 0 ? "empty" : n < 5 ? "some" : "full");

/** 10개를 넘긴 사고 수만큼 종이가 바닥으로 나온다 (HANDOFF 6.6). 11번째 사고에 첫 종이 */
export const paperCount = (n: number, slots: number): number =>
  Math.min(slots, Math.max(0, n - 10));

/** 무재해 분. 전광판은 4자리라 9999 에서 멈춘다. 읽을 수 없는 시각은 0 */
export function safeMinutes(since: string, nowMs: number): number {
  const t = Date.parse(since);
  if (Number.isNaN(t)) return 0;
  return Math.min(9999, Math.max(0, Math.floor((nowMs - t) / 60_000)));
}

/** 현지 시각 06–17 day, 17–22 dusk, 22–06 night */
export const windowState = (hour: number): string =>
  hour >= 6 && hour < 17 ? "day" : hour >= 17 && hour < 22 ? "dusk" : "night";

export type NoteColor = "yellow" | "pink" | "blue" | "green";

/**
 * 할 일 → 칸반 슬롯. 열 우선(할 일 0–2, 진행 3–5, 완료 6–8). 열이 넘치면 할 일·진행은 앞쪽,
 * 완료는 최근 것(뒤쪽) 3개만 둔다. 할 일은 노랑·분홍을 번갈아 (HANDOFF 6.6).
 */
export function kanbanNotes(items: TodoItem[]): { slot: number; color: NoteColor }[] {
  const col = (s: TodoItem["status"]) => items.filter((t) => t.status === s);
  const out: { slot: number; color: NoteColor }[] = [];
  col("Pending")
    .slice(0, 3)
    .forEach((_, i) => out.push({ slot: i, color: i % 2 ? "pink" : "yellow" }));
  col("InProgress")
    .slice(0, 3)
    .forEach((_, i) => out.push({ slot: 3 + i, color: "blue" }));
  col("Done")
    .slice(-3)
    .forEach((_, i) => out.push({ slot: 6 + i, color: "green" }));
  return out;
}

// 평면도에 없어 코드가 놓는 자리 (격자). 도장은 도장 책상 위, 상자는 출고장 바닥, 폭죽은 출고장 위
// ponytail: 도장 자리와 효과 이름이 하나라 팀이 다른 도장이 2초 안에 연달아 오면 앞것이 사라진다, 겹치는 사례가 나오면 팀별 자리로
const STAMP_AT = { gx: 15.1, gy: 3.25, z: 9 };
const BOX_SLOTS: [number, number][] = [
  [3.9, 14.6],
  [3.9, 15.4],
  [1.5, 15.3],
];
const RETURN_AT = { gx: 2.6, gy: 15.4 };
const FALL_Z = 14;
const FALL_MS = 250;
const FIREWORKS_AT = { gx: 2.5, gy: 12.5, z: 34 };
// 트럭은 출고장 앞 바닥 밖에서 +gy 쪽으로 빠져나간다 (office.json truck)
const TRUCK_FROM = { gx: 1.5, gy: 16.6 };
const TRUCK_TO_GY = 22;
// 회의 탁자 위 (탁자 높이 z=9): 명함과 펼친 매뉴얼
const CARD_AT = { gx: 19.2, gy: 14.0, z: 9 };
const MANUAL_AT = { gx: 20.6, gy: 14.0, z: 9 };
// 미팅실 화면의 밝은 띠 중심과 폭 (meeting_screen 소품 위치 기준 1× px)
const BAND = { x: 10, y: -5, w: 18 };
// 사다리는 사규 액자 아래 벽에 기대 놓는다 (액자 위치에서의 격자 차이). 액자 옆 화분(부장 책상)에 가리지 않게 화분 오른쪽
const LADDER_AT = { dgx: 0.85, dgy: 0.3 };
// 효과는 다른 소품 위에 그린다
const FX_DEPTH = 80;

// 피자 상자는 탁자 위 한 곳에 쌓이고 상자마다 0.01 씩 앞으로 (render_scene)
const PIZZA_DEPTH = 26.21;
// 종이·피자는 월드의 평면도 소품과 겹쳐도 같은 depth 면 뒤에 오게 순번을 크게
const LOOSE_SEQ = 5000;

interface PropMeta {
  slots?: number[][];
  footprint?: [number, number];
  height?: number;
  stack?: { gx: number; gy: number; z0: number; dz: number; max: number };
  screen?: Record<"bl" | "br" | "tl" | "tr", [number, number]>;
  lineColors?: string[];
  digitOrigin?: [number, number];
  digitStep?: [number, number];
  maxDigits?: number;
}

type Frames = Record<string, { anchorPx: [number, number] } | undefined>;

/** 장면을 다시 지어도 남겨야 하는 것. DTO 에 없어 cue 로만 알 수 있다 */
export interface Remembered {
  /** 백그라운드 명령 중인 직원 (작업표 한 장이 보인다) */
  tickets: Set<string>;
  /** 마지막 테스트 결과 */
  board: "pass" | "fail" | undefined;
}

interface Fx {
  until: number;
  parts: Sprite[];
  tick?: (now: number) => void;
}

type ToCss = (x: number, y: number) => { left: number; top: number };

/** 미팅실 화면에 쓰는 거래처 상호. 글자는 DOM 이라 장면을 새로 지어도 이 층은 남는다 (HANDOFF 4.4) */
export class ScreenLabel {
  private readonly el: HTMLElement;
  private at = { x: 0, y: 0, w: 0 };
  private toCss: ToCss = (x, y) => ({ left: x, top: y });

  constructor(overlay: HTMLElement) {
    this.el = overlay.ownerDocument.createElement("div");
    this.el.setAttribute("aria-hidden", "true");
    this.el.hidden = true;
    Object.assign(this.el.style, {
      position: "absolute",
      transform: "translate(-50%, -50%)",
      font: "var(--ow-text-bubble)",
      color: "#18181b",
      whiteSpace: "nowrap",
      overflow: "hidden",
      textOverflow: "ellipsis",
      pointerEvents: "none",
    });
    overlay.append(this.el);
  }

  /** 글이 없으면 숨긴다. 좌표는 1× 캔버스 */
  set(text: string | null, x: number, y: number, w: number): void {
    this.at = { x, y, w };
    this.el.hidden = text === null;
    if (this.el.textContent !== (text ?? "")) this.el.textContent = text ?? "";
    this.put();
  }

  place(toCss: ToCss): void {
    this.toCss = toCss;
    this.put();
  }

  private put(): void {
    const p = this.toCss(this.at.x, this.at.y);
    this.el.style.left = `${p.left}px`;
    this.el.style.top = `${p.top}px`;
    // 띠보다 길면 말줄임으로 자른다
    this.el.style.maxWidth = `${this.toCss(this.at.w, 0).left}px`;
  }
}

export class PropsLayer {
  private readonly meta: Record<string, PropMeta>;
  private readonly pl: Map<string, Floorplan["layouts"][Kind]["placements"][number]>;
  private readonly drawn = new Map<string, string>();
  private readonly groups = new Map<string, { destroy(): void }[]>();
  private readonly bossSeats: BossSeat[];
  private fx: BossFx[] = [];
  private ringUntil = 0;
  private doorUntil = 0;
  private incident = false;
  private game = false;
  /** 분갈이 중이라 비워 둔 화분: 끝나는 시각과, 끝나면 보여야 하는지(중소기업은 팀장석이 비면 없다) */
  private readonly repots = new Map<string, { until: number; on: boolean }>();
  private lab: LabFx = { shell: false, slow: false, test: false, boxes: 0 };
  private readonly effects = new Map<string, Fx>();

  constructor(
    private readonly fp: Floorplan,
    private readonly sheet: Spritesheet,
    private readonly kind: Kind,
    private readonly target: PropsTarget,
    readonly memory: Remembered = { tickets: new Set(), board: undefined },
    private readonly label: ScreenLabel | null = null,
  ) {
    this.meta = (
      sheet.data as unknown as { owlfice: { props: Record<string, PropMeta> } }
    ).owlfice.props;
    this.pl = new Map(fp.layouts[kind].placements.map((p) => [p.id, p]));
    this.bossSeats = fp.layouts[kind].seats.boss;
  }

  /** 외근에서 돌아오는 직원이 문을 연다 */
  openDoor(nowMs: number): void {
    this.doorUntil = nowMs + DOOR_OPEN_MS;
    this.tick(nowMs);
  }

  /** 사장님 방문 때 인터폰이 1초 울린다 */
  ring(nowMs: number): void {
    this.ringUntil = nowMs + RING_MS;
    this.tick(nowMs);
  }

  /** 김·화면보호기·벨은 프레임이 바뀌어서 매 프레임 불린다. 상태는 update 가 정한 것을 쓴다 */
  tick(nowMs: number): void {
    this.bossSeats.forEach((seat, i) => {
      const fx = this.fx[i];
      if (seat.mug) this.state(seat.mug, fx?.steam ? `steam_${blink(nowMs, MUG_FPS)}` : undefined);
      const duck = seat.duck && this.target.driven.get(seat.duck);
      if (duck) duck.visible = !fx?.duck;
      for (const m of seat.monitors ?? [])
        this.state(m, fx?.saver ? `saver_${blink(nowMs, SAVER_FPS)}` : undefined);
    });
    this.state("interphone", nowMs < this.ringUntil ? `ring_${blink(nowMs, RING_FPS)}` : undefined);
    this.state("door", nowMs < this.doorUntil ? "open" : undefined);
    // 움직임 줄이기: 반복 프레임은 첫 장에 멈춘다
    const still = reducedMotion();
    const f = (fps: number) => (still ? 0 : blink(nowMs, fps));
    for (const id of ["rack_0", "rack_1"])
      this.state(id, this.lab.shell ? `led_${f(RACK_FPS)}` : undefined);
    this.state("game_tv", this.game ? `game_${f(GAME_FPS)}` : undefined);
    this.state("fan", this.lab.slow ? `spin_${f(FAN_FPS)}` : undefined);
    this.state("test_machine", this.lab.test ? `run_${f(MACHINE_FPS)}` : undefined);
    this.state("beacon", this.incident ? `on_${f(BEACON_FPS)}` : undefined);
    for (const [id, r] of this.repots) {
      const sprite = this.target.driven.get(id);
      const done = nowMs >= r.until;
      if (done) this.repots.delete(id);
      if (sprite) sprite.visible = done && r.on;
    }
    for (const [name, e] of this.effects)
      if (nowMs >= e.until) {
        for (const p of e.parts) p.destroy();
        this.effects.delete(name);
      } else e.tick?.(nowMs);
  }

  /**
   * 한 번 일어나는 소품 연출: 도장·트럭·반송·폭죽, 작업표 (S15 S16 S30 S31 S48, S61).
   * 사무실 전환·조용한 재구성이면 부르지 않는다 (ActorLayer 가 거른다).
   */
  cue(c: CueDto, now: number, office: OfficeDto | null = null): void {
    if (c.kind === "Paper") {
      const key = empKey(c.teamId, c.agent);
      if (c.on) this.memory.tickets.add(key);
      else this.memory.tickets.delete(key);
      this.showTicket();
    } else if (c.kind === "Stamp") this.stamp(c, now);
    else if (c.kind === "Truck") this.truck(now);
    else if (c.kind === "Return") this.returned(now);
    else if (c.kind === "Fireworks") this.fireworks(now);
    else if (c.kind === "Repot") this.repot(c.teamId, now, office);
  }

  /** 제목이 바뀌면(S39) 직원이 사규 액자로 가는 동안 `at` 에 사다리가 놓여 LADDER_MS 뒤 걷힌다 */
  ladder(at: number, now: number): void {
    const frame = this.pl.get("rules_frame");
    if (!frame) return;
    const s = this.fxSprite(
      "ladder/default",
      frame.gx + LADDER_AT.dgx,
      frame.gy + LADDER_AT.dgy,
      0,
    );
    s.zIndex = zIndexOf(frame.depth + 0.02, LOOSE_SEQ);
    this.addFx("ladder", at + LADDER_MS, [s], now, (t) => void (s.visible = t >= at));
  }

  /**
   * 분갈이(S38): 새 화분(s0)은 상태가 이미 정했다. 그 전에 REPOT_MS 동안 화분이 비어 있다 나온다.
   * 움직임 줄이기면 결과만.
   */
  private repot(teamId: string, now: number, office: OfficeDto | null): void {
    if (reducedMotion()) return;
    // 이 cue 와 함께 온 사무실로 찾는다. update 는 아직 이전 스냅샷을 그리고 있다
    const teams = office?.teams ?? [];
    const team = teams.find((t) => t.teamId === teamId);
    if (!team) return;
    const slot = this.kind === "large" ? (team === teams[0] ? 0 : null) : team.deskSlot;
    if (slot === null || slot > 3) return;
    const id = `plant_boss_${slot}`;
    // 연달아 오면 앞 분갈이가 숨긴 상태를 "보임"으로 읽지 않게 앞의 값을 이어 쓴다
    const on = this.repots.get(id)?.on ?? this.target.driven.get(id)?.visible ?? true;
    this.repots.set(id, { until: now + REPOT_MS, on });
    this.tick(now);
  }

  /** 사무실 전환: 지난 사무실의 작업표·테스트 결과와 진행 중인 효과를 지운다 */
  reset(): void {
    this.memory.tickets.clear();
    this.memory.board = undefined;
    for (const [id, r] of this.repots) {
      const sprite = this.target.driven.get(id);
      if (sprite) sprite.visible = r.on;
    }
    this.repots.clear();
    for (const e of this.effects.values()) for (const p of e.parts) p.destroy();
    this.effects.clear();
  }

  private stamp(c: Extract<CueDto, { kind: "Stamp" }>, now: number): void {
    const fail = c.failed > 0;
    this.memory.board = fail ? "fail" : "pass";
    this.state("test_board", this.memory.board);
    const key = `stamp_${fail ? "fail" : "pass"}${c.estimated ? "_est" : ""}/default`;
    const { gx, gy, z } = STAMP_AT;
    const s = this.fxSprite(key, gx, gy, z);
    // 도장 책상과 같은 depth 면 책상 뒤로 가려진다
    s.zIndex = zIndexOf((this.pl.get("stamp_table")?.depth ?? 17) + 0.01, LOOSE_SEQ);
    this.addFx("stamp", now + STAMP_RESULT_MS, [s], now);
  }

  private truck(now: number): void {
    const s = this.fxSprite("truck/drive_0", TRUCK_FROM.gx, TRUCK_FROM.gy, 0);
    s.zIndex = zIndexOf(FX_DEPTH, LOOSE_SEQ);
    // 움직임 줄이기: 제자리에서 같은 시간 동안만 보인다
    if (reducedMotion()) return this.addFx("truck", now + TRUCK_MS, [s], now);
    // 프레임마다 기준점이 같아 텍스처만 바꾼다
    this.addFx("truck", now + TRUCK_MS, [s], now, (t) => {
      const k = (t - now) / TRUCK_MS;
      const p = gridToScreen(TRUCK_FROM.gx, TRUCK_FROM.gy + (TRUCK_TO_GY - TRUCK_FROM.gy) * k);
      s.position.set(Math.round(p.x), Math.round(p.y));
      s.texture =
        this.sheet.textures[`truck/drive_${Math.floor(((t - now) * TRUCK_FPS) / 1000) % 2}`];
    });
  }

  private returned(now: number): void {
    const { gx, gy } = RETURN_AT;
    const box = this.fxSprite("box_taped/default", gx, gy, 0);
    const note = this.fxSprite("note_return/default", gx, gy, 0);
    box.zIndex = zIndexOf(gx + gy + 0.65, LOOSE_SEQ);
    note.zIndex = box.zIndex + 1;
    // 상자가 떨어져 바닥에 닿는다. 움직임 줄이기면 바로 바닥
    const lay = (t: number) => {
      const k = reducedMotion() ? 1 : Math.min(1, (t - now) / FALL_MS);
      const z = FALL_Z * (1 - k);
      for (const [s, up] of [
        [box, 0],
        [note, 8],
      ] as const) {
        const p = gridToScreen(gx, gy, z + up);
        s.position.set(Math.round(p.x), Math.round(p.y));
      }
    };
    this.addFx("return", now + RETURN_MS, [box, note], now, lay);
  }

  private fireworks(now: number): void {
    const { gx, gy, z } = FIREWORKS_AT;
    const s = this.fxSprite("fireworks/0", gx, gy, z);
    s.zIndex = zIndexOf(FX_DEPTH, LOOSE_SEQ + 1);
    // 움직임 줄이기: 마지막 프레임을 같은 시간 동안만 보인다
    if (reducedMotion()) {
      s.texture = this.sheet.textures["fireworks/2"];
      return this.addFx("fireworks", now + FIREWORKS_MS, [s], now);
    }
    this.addFx("fireworks", now + FIREWORKS_MS, [s], now, (t) => {
      const i = Math.min(2, Math.floor(((t - now) * FIREWORKS_FPS) / 1000));
      s.texture = this.sheet.textures[`fireworks/${i}`];
    });
  }

  private fxSprite(key: string, gx: number, gy: number, z: number): Sprite {
    const p = gridToScreen(gx, gy, z);
    const s = placeSprite(this.sheet, key, Math.round(p.x), Math.round(p.y));
    this.target.world.addChild(s);
    return s;
  }

  /** 같은 이름의 효과가 진행 중이면 새것으로 바꾼다 */
  private addFx(
    name: string,
    until: number,
    parts: Sprite[],
    now: number,
    tick?: (now: number) => void,
  ): void {
    for (const p of this.effects.get(name)?.parts ?? []) p.destroy();
    this.effects.set(name, { until, parts, tick });
    tick?.(now);
  }

  // ponytail: 작업표는 한 장만 보인다 (평면도에 job_ticket 이 하나), 명령 수만큼 쌓기는 아트가 생기면
  private showTicket(): void {
    const ticket = this.target.driven.get("job_ticket");
    if (ticket) ticket.visible = this.memory.tickets.size > 0;
  }

  private updateBoxes(n: number): void {
    const k = Math.min(BOX_SLOTS.length, n);
    if (!this.changed("boxes", String(k))) return;
    this.replace(
      "boxes",
      BOX_SLOTS.slice(0, k).map(([gx, gy], i) => {
        const s = this.fxSprite("box_taped/default", gx, gy, 0);
        s.zIndex = zIndexOf(gx + gy + 0.65, LOOSE_SEQ + i);
        return s;
      }),
    );
  }

  /** office 가 null 이면 평면도 기본 상태 (그림을 더하지 않는다) */
  update(office: OfficeDto | null, nowMs: number): void {
    const shared = office?.shared;
    this.state("ci_light", shared ? shared.light.toLowerCase() : undefined);
    this.state("window", windowState(new Date(nowMs).getHours()));
    this.state("trash", shared ? trashState(shared.trash) : undefined);
    const ramen = this.target.driven.get("ramen");
    if (ramen) ramen.visible = shared?.night ?? false;

    this.updatePlants(office);
    this.updateKanban(shared?.kanban ?? []);
    this.updateDigits(shared ? String(safeMinutes(shared.safeSince, nowMs)) : "");
    this.updatePapers(shared?.trash ?? 0);
    this.updatePizza(shared?.pizza ?? 0);
    this.updateTv(office);
    this.updateMagnets(office);
    this.game = gameOn(office, this.kind);
    this.lab = labFx(office);
    this.updateBoxes(this.lab.boxes);
    this.state("terminal", this.lab.shell ? "on" : undefined);
    this.state("test_board", this.memory.board);
    this.showTicket();
    this.updateRooms(office);
    // 대기업은 팀 하나가 팀장석 하나, 중소기업은 팀장석 번호가 deskSlot
    this.fx = this.bossSeats.map((_, i) =>
      bossFx(
        this.kind === "large" ? office?.teams[0] : office?.teams.find((t) => t.deskSlot === i),
      ),
    );
    this.tick(nowMs);
  }

  /** 평면도 소품의 상태를 바꾼다. undefined 는 평면도 기본 상태 */
  private state(id: string, state: string | undefined): void {
    const sprite = this.target.driven.get(id);
    const pl = this.pl.get(id);
    if (!sprite || !pl) return;
    const key = `${pl.sprite}/${state ?? pl.state}`;
    // 서명은 텍스처를 붙인 뒤에 기록한다. 프레임이 없어 throw 한 채 기록되면 다음 update 가 건너뛴다
    if (this.drawn.get(`state:${id}`) === key) return;
    const frame = (this.sheet.data.frames as unknown as Frames)[key];
    if (!frame) throw new Error(`office.json 에 프레임 ${key} 없음`);
    sprite.texture = this.sheet.textures[key];
    // 화분처럼 단계마다 기준점이 다른 소품이 있다
    sprite.pivot.set(frame.anchorPx[0], frame.anchorPx[1]);
    this.drawn.set(`state:${id}`, key);
  }

  /** 미팅실·리뷰실·장애 상황실·외근 팻말. 전부 지금 상태로 정해지고 cue 는 없다 */
  private updateRooms(office: OfficeDto | null): void {
    const fx = roomFx(office);
    this.incident = office?.shared.incident ?? false;
    this.state("meeting_screen", fx.vendor !== null ? "on" : undefined);
    this.state("review_screen", fx.pens !== null ? "code" : undefined);
    this.state("whiteboard", this.incident ? "scribble" : undefined);
    this.state("incident_table", this.incident ? "on" : undefined);
    const ms = this.pl.get("meeting_screen");
    if (ms) this.label?.set(fx.vendor, ms.x + BAND.x, ms.y + BAND.y, BAND.w);
    this.updateMeetingTable(fx);
    this.updatePens(fx.pens ?? 0);
    this.updateAway(office);
  }

  private updateMeetingTable(fx: RoomFx): void {
    const things = [
      ...(fx.vendor !== null ? [["business_card", CARD_AT] as const] : []),
      ...(fx.skill ? [["manual_open", MANUAL_AT] as const] : []),
    ];
    if (!this.changed("meeting", things.map(([k]) => k).join())) return;
    const depth = (this.pl.get("meeting_table")?.depth ?? 33.75) + 0.01;
    this.replace(
      "meeting",
      things.map(([k, { gx, gy, z }], i) => {
        const s = this.fxSprite(`${k}/default`, gx, gy, z);
        s.zIndex = zIndexOf(depth, LOOSE_SEQ + i);
        return s;
      }),
    );
  }

  /** 지적마다 리뷰 화면 위에 빨간 포스트잇. 화면 슬롯 수까지만 */
  private updatePens(pens: number): void {
    // ponytail: 승인 문구(review_screen approved)는 DTO 에 신호가 없어 못 켠다, T25b 가 백엔드 스냅샷 값을 주면
    const rs = this.pl.get("review_screen");
    const slots = this.meta.review_screen?.slots ?? [];
    const n = Math.min(slots.length, pens);
    if (!rs || !this.changed("pens", String(n))) return;
    const screenZ = this.target.driven.get("review_screen")?.zIndex ?? 0;
    this.replace(
      "pens",
      slots.slice(0, n).map(([dx, dy]) => {
        const s = placeSprite(this.sheet, "postit_red/default", rs.x + dx, rs.y + dy);
        // 화면 바로 위. 같은 depth 의 다른 소품에 가려지지 않게 화면 키 바로 뒤
        s.zIndex = screenZ + 0.5;
        this.target.world.addChild(s);
        return s;
      }),
    );
  }

  /** 외근 중인 직원의 자리 책상 위에 팻말 (S23). 퇴장한 서브와 팀장석이 없는 팀은 뺀다 */
  private updateAway(office: OfficeDto | null): void {
    const desks = [...seated(this.fp, this.kind, office)].flatMap(({ emp, spot }) =>
      emp.activity.room === "Outside" && spot?.desk ? [spot.desk] : [],
    );
    if (!this.changed("away", desks.join())) return;
    this.replace(
      "away",
      desks.flatMap((id) => {
        const pl = this.pl.get(id);
        const m = pl && this.meta[pl.sprite];
        if (!pl || !m?.footprint) return [];
        // 책상 윗면 가운데
        const s = this.fxSprite(
          "sign_away/default",
          pl.gx + m.footprint[0] / 2,
          pl.gy + m.footprint[1] / 2,
          m.height ?? 0,
        );
        s.zIndex = zIndexOf(pl.depth + 0.02, LOOSE_SEQ);
        return [s];
      }),
    );
  }

  private updatePlants(office: OfficeDto | null): void {
    if (this.kind === "large") {
      this.state("plant_boss_0", plantState(office?.teams[0]?.props.plant ?? null));
      return;
    }
    // 중소기업: 팀장석(deskSlot 0~3)마다 그 팀 부장의 화분. 비어 있는 자리는 숨긴다
    for (let i = 0; i < 4; i++) {
      const id = `plant_boss_${i}`;
      const team = office?.teams.find((t) => t.deskSlot === i);
      const sprite = this.target.driven.get(id);
      const r = this.repots.get(id);
      if (r) r.on = !!team;
      if (sprite) sprite.visible = !!team && !r;
      this.state(id, plantState(team?.props.plant ?? null));
    }
  }

  private updateKanban(items: TodoItem[]): void {
    const notes = kanbanNotes(items);
    const kb = this.pl.get("kanban");
    if (!kb || !this.changed("kanban", JSON.stringify(notes))) return;
    const slots = this.meta[kb.sprite].slots ?? [];
    this.replace(
      "kanban",
      notes.map(({ slot, color }) => {
        const s = placeSprite(
          this.sheet,
          `wall_note/${color}`,
          kb.x + slots[slot][0],
          kb.y + slots[slot][1],
        );
        this.target.wall.addChild(s);
        return s;
      }),
    );
  }

  private updateMagnets(office: OfficeDto | null): void {
    const board = this.pl.get("attendance");
    // 근태판 slots 는 [칸][줄][dx, dy] 라 한 겹 깊다 (PropMeta.slots 는 다른 소품의 [i][dx, dy])
    const slots = this.meta.attendance?.slots as unknown as number[][][] | undefined;
    if (!board || !slots) return;
    const list = magnets(office, this.kind, slots[0].length);
    if (!this.changed("magnets", JSON.stringify(list))) return;
    this.replace(
      "magnets",
      list.map(({ col, line, color }) => {
        const [dx, dy] = slots[col][line];
        const s = placeSprite(this.sheet, `magnet/${color}`, board.x + dx, board.y + dy);
        this.target.wall.addChild(s);
        return s;
      }),
    );
  }

  private updateDigits(text: string): void {
    const board = this.pl.get("safety_board");
    const boardSprite = this.target.driven.get("safety_board");
    if (!board || !boardSprite || !this.changed("digits", text)) return;
    const { digitOrigin: o, digitStep: d } = this.meta.safety_board;
    if (!o || !d) throw new Error("office.json safety_board 에 digitOrigin·digitStep 없음");
    this.replace(
      "digits",
      text.split("").map((ch, n) => {
        const s = placeSprite(
          this.sheet,
          `digit/${ch}`,
          board.x + o[0] + n * d[0],
          board.y + o[1] + n * d[1],
        );
        // 전광판 바로 위. 같은 depth 의 다른 소품에 가려지지 않게 전광판 키 바로 뒤
        s.zIndex = boardSprite.zIndex + 0.5;
        this.target.world.addChild(s);
        return s;
      }),
    );
  }

  private updatePapers(trash: number): void {
    const slots = this.meta.paper_ball.slots ?? [];
    const n = paperCount(trash, slots.length);
    if (!this.changed("papers", String(n))) return;
    this.replace(
      "papers",
      slots.slice(0, n).map(([gx, gy], i) => {
        const p = gridToScreen(gx, gy);
        const s = placeSprite(this.sheet, "paper_ball/default", Math.round(p.x), Math.round(p.y));
        s.zIndex = zIndexOf(gx + gy, LOOSE_SEQ + i);
        this.target.world.addChild(s);
        return s;
      }),
    );
  }

  private updatePizza(pizza: number): void {
    const stack = this.meta.pizza_box.stack;
    if (!stack) throw new Error("office.json pizza_box 에 stack 없음");
    const n = Math.min(stack.max, Math.max(0, pizza));
    if (!this.changed("pizza", String(n))) return;
    this.replace(
      "pizza",
      Array.from({ length: n }, (_, k) => {
        const p = gridToScreen(stack.gx, stack.gy, stack.z0 + stack.dz * k);
        const s = placeSprite(this.sheet, "pizza_box/default", Math.round(p.x), Math.round(p.y));
        s.zIndex = zIndexOf(PIZZA_DEPTH + k * 0.01, LOOSE_SEQ + k);
        this.target.world.addChild(s);
        return s;
      }),
    );
  }

  /**
   * 팀마다 컨텍스트 사용률 선 하나. 중소기업은 deskSlot 0~3 이 곧 색 번호라 팀 순서가 바뀌어도 색이 같다.
   * 대기업은 팀 하나(teams[0])라 첫 색.
   * ponytail: DTO 에 시간축 기록이 없어 지금 값을 오른쪽 끝(u=1)에 점 하나로만 찍는다. 기록이 생기면 u 로 펴서 선으로.
   */
  private updateTv(office: OfficeDto | null): void {
    const tv = this.pl.get("tv");
    const { screen: q, lineColors: colors } = this.meta.tv;
    if (!tv || !q || !colors) return;
    const teams = office?.teams ?? [];
    const picked =
      this.kind === "large"
        ? teams.slice(0, 1).map((t) => ({ t, slot: 0 }))
        : teams.flatMap((t) =>
            t.deskSlot !== null && t.deskSlot < colors.length ? [{ t, slot: t.deskSlot }] : [],
          );
    const dots = picked
      .map(({ t, slot }) => ({ v: t.props.plant, color: colors[slot] }))
      .flatMap(({ v, color }) => (v === null ? [] : [{ v: Math.min(1, Math.max(0, v)), color }]));
    if (!this.changed("tv", JSON.stringify(dots))) return;
    const g = new Graphics();
    for (const { v, color } of dots) {
      // 오른쪽 아래 꼭짓점 br 에서 위 변 방향(tl - bl)으로 v 만큼
      const x = tv.x + q.br[0] + v * (q.tl[0] - q.bl[0]);
      const y = tv.y + q.br[1] + v * (q.tl[1] - q.bl[1]);
      const [r, gr, b] = hexToRgb(color);
      g.rect(Math.round(x), Math.round(y), 1, 1).fill((r << 16) | (gr << 8) | b);
    }
    this.target.wall.addChild(g);
    this.replace("tv", [g]);
  }

  private changed(name: string, sig: string): boolean {
    if (this.drawn.get(name) === sig) return false;
    this.drawn.set(name, sig);
    return true;
  }

  private replace(name: string, next: { destroy(): void }[]): void {
    for (const old of this.groups.get(name) ?? []) old.destroy();
    this.groups.set(name, next);
  }
}
