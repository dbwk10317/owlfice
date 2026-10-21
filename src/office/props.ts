// 소품 상태: 신호등·화분·전광판·휴지통·칸반·피자·창밖·TV (HANDOFF 5, 6.5, 6.6, PLAN 3.4, 4.7.9).
// 값 → 상태 이름은 순수 함수, PropsLayer 는 바뀐 소품만 다시 그린다. 움직임은 cue 몫이다.
import { Graphics, type Spritesheet } from "pixi.js";
import type { OfficeDto, TodoItem } from "../types/ipc.ts";
import { gridToScreen, hexToRgb, zIndexOf, type Floorplan, type Kind } from "./floorplan.ts";
import type { Scene } from "./renderer.ts";
import { placeSprite } from "./sprites.ts";

type PropsTarget = Pick<Scene, "wall" | "world" | "driven">;

/** 평면도에서 상태나 보임을 코드가 바꾸는 소품. 숨겨진 채 시작하는 것(ramen, 중소기업 화분)도 장면에 만든다 */
export const DRIVEN_PROP = /^(ci_light|window|trash|safety_board|ramen|plant_boss_\d)$/;

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

// 피자 상자는 탁자 위 한 곳에 쌓이고 상자마다 0.01 씩 앞으로 (render_scene)
const PIZZA_DEPTH = 26.21;
// 종이·피자는 월드의 평면도 소품과 겹쳐도 같은 depth 면 뒤에 오게 순번을 크게
const LOOSE_SEQ = 5000;

interface PropMeta {
  slots?: number[][];
  stack?: { gx: number; gy: number; z0: number; dz: number; max: number };
  screen?: Record<"bl" | "br" | "tl" | "tr", [number, number]>;
  lineColors?: string[];
  digitOrigin?: [number, number];
  digitStep?: [number, number];
  maxDigits?: number;
}

type Frames = Record<string, { anchorPx: [number, number] } | undefined>;

export class PropsLayer {
  private readonly meta: Record<string, PropMeta>;
  private readonly pl: Map<string, Floorplan["layouts"][Kind]["placements"][number]>;
  private readonly drawn = new Map<string, string>();
  private readonly groups = new Map<string, { destroy(): void }[]>();

  constructor(
    fp: Floorplan,
    private readonly sheet: Spritesheet,
    private readonly kind: Kind,
    private readonly target: PropsTarget,
  ) {
    this.meta = (
      sheet.data as unknown as { owlfice: { props: Record<string, PropMeta> } }
    ).owlfice.props;
    this.pl = new Map(fp.layouts[kind].placements.map((p) => [p.id, p]));
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
      if (sprite) sprite.visible = !!team;
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
