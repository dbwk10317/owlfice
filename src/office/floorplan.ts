// design/sprites/floorplan.json 을 읽고 격자·방·배치를 푼다 (HANDOFF 6.1, 6.5).
// 그리기 규칙의 정답은 design/tools/owlfice_art.py 의 render_scene 이다.

export const CANVAS_W = 520;
export const CANVAS_H = 320;
const ORIGIN_X = 212;
const ORIGIN_Y = 60;

export type Kind = "large" | "small";
export type Theme = "claude" | "codex" | "gemini";
export type Layer = "decal" | "wall" | "window" | "world" | "fx" | "overlay";
export type LightName = "night" | "dim" | "off";

export interface Placement {
  id: string;
  sprite: string;
  state: string;
  gx: number;
  gy: number;
  z: number;
  depth: number;
  x: number;
  y: number;
  layer: Layer;
  visible?: boolean;
  themed?: boolean;
}

export interface Seat {
  id: string;
  gx: number;
  gy: number;
  depth: number;
  desk: string;
  monitors?: string[];
  partition?: string;
  plant?: string;
}

export interface Room {
  name: string;
  rect: [number, number, number, number];
  floor: string;
  label: [number, number];
  entry: [number, number];
}

export interface Layout {
  placements: Placement[];
  seats: { boss: Seat[]; dev: Seat[]; temp: Seat[]; spare: Seat[] };
}

export interface Floorplan {
  grid: { cols: number; rows: number };
  rooms: Record<string, Room>;
  layouts: Record<Kind, Layout>;
  stands: Record<string, number[][]>;
  themes: Record<Theme, { cubicle: string; uniform: string; sofa: string; accent: string }>;
  teamTints: string[];
  lightTints: Record<LightName, string>;
}

/** fetch 한 JSON 을 받는다. 이 파일의 상수와 어긋난 평면도는 화면이 조용히 틀어지므로 여기서 막는다. */
export function parseFloorplan(raw: unknown): Floorplan {
  const fp = raw as Record<string, any>;
  const ok =
    fp?.version === 1 &&
    fp.canvas1x?.w === CANVAS_W &&
    fp.canvas1x?.h === CANVAS_H &&
    fp.origin?.x === ORIGIN_X &&
    fp.origin?.y === ORIGIN_Y &&
    Array.isArray(fp.layouts?.large?.placements) &&
    Array.isArray(fp.layouts?.small?.placements) &&
    typeof fp.rooms === "object" &&
    typeof fp.themes === "object";
  if (!ok)
    throw new Error("floorplan.json 형식이 렌더러와 맞지 않는다 (version·canvas1x·origin·layouts)");
  return fp as Floorplan;
}

/** 격자 → 1× 화면 좌표 (HANDOFF 6.1). 반올림은 부르는 쪽이 한다. */
export function gridToScreen(gx: number, gy: number, z = 0): { x: number; y: number } {
  return { x: ORIGIN_X + (gx - gy) * 12, y: ORIGIN_Y + (gx + gy) * 6 - z };
}

/** 동 글자(PLAN 2.1) → 테마. A·B Claude, C·D Codex, E·F Antigravity(Gemini). */
export function themeOf(building: string | undefined): Theme {
  if (building === "C" || building === "D") return "codex";
  if (building === "E" || building === "F") return "gemini";
  return "claude";
}

export function roomAt(fp: Floorplan, i: number, j: number): string | undefined {
  for (const [key, r] of Object.entries(fp.rooms)) {
    const [a, b, c, d] = r.rect;
    if (a <= i && i < c && b <= j && j < d) return key;
  }
  return undefined;
}

/** floorplan.floorRule 그대로. */
export function tileFrame(
  fp: Floorplan,
  i: number,
  j: number,
  theme: Theme,
  incident: boolean,
): string {
  const key = roomAt(fp, i, j);
  let floor = key ? fp.rooms[key].floor : "dev";
  if (floor === "boss") floor = `boss_${theme}`;
  else if (floor === "incident") floor = incident ? "incident_on" : "incident_off";
  return `tile/${floor}/${(i + j) % 2 === 0 ? "a" : "b"}`;
}

/** `themed` 소품은 이름 끝 테마(`_claude`)를 현재 테마로 바꾼다. */
export function spriteName(pl: Placement, theme: Theme): string {
  return pl.themed ? pl.sprite.replace(/_[^_]+$/, `_${theme}`) : pl.sprite;
}

/**
 * 월드 정렬 키. HANDOFF 6.2 의 `depth × 1000 + 순번` 은 depth 차가 0.01(=10)인 소품 사이에서
 * 순번 10 이상이 끼면 순서가 뒤집혀, 순번 칸을 넉넉히 둔다. depth 는 소수 셋째 자리까지라 정수로 맞춘다.
 */
export function zIndexOf(depth: number, seq: number): number {
  return Math.round(depth * 1000) * 10000 + seq;
}

export function hexToNumber(hex: string): number {
  return Number.parseInt(hex.slice(1), 16);
}

/** 파티션 곱하기 색 (HANDOFF 6.5). */
// ponytail: 중소기업 좌석의 팀 색은 직원을 앉히는 T20 에서 넣는다, 지금은 빈자리 색(테마 cubicle)만
export function partitionTint(fp: Floorplan, theme: Theme): string {
  return fp.themes[theme].cubicle;
}
