import { expect, test } from "vite-plus/test";
import rawFloorplan from "../../design/sprites/floorplan.json";
import office from "../../design/sprites/office.json";
import {
  gridToScreen,
  parseFloorplan,
  roomAt,
  spriteName,
  tileFrame,
  zIndexOf,
  type Kind,
  type Theme,
} from "./floorplan.ts";

const fp = parseFloorplan(rawFloorplan);
const KINDS: Kind[] = ["large", "small"];
const THEMES: Theme[] = ["claude", "codex", "gemini"];

test("격자 → 화면은 HANDOFF 6.1 식", () => {
  expect(gridToScreen(0, 0)).toEqual({ x: 212, y: 60 });
  expect(gridToScreen(24, 0)).toEqual({ x: 500, y: 204 });
  expect(gridToScreen(0, 16)).toEqual({ x: 20, y: 156 });
  expect(gridToScreen(2, 1, 10)).toEqual({ x: 224, y: 68 });
});

test("평면도에 적힌 소품 좌표가 같은 식을 반올림한 값이다", () => {
  for (const kind of KINDS)
    for (const pl of fp.layouts[kind].placements) {
      const p = gridToScreen(pl.gx, pl.gy, pl.z);
      expect(Math.abs(pl.x - p.x), pl.id).toBeLessThanOrEqual(0.5);
      expect(Math.abs(pl.y - p.y), pl.id).toBeLessThanOrEqual(0.5);
    }
});

test("깊이 정렬은 (depth, 배열 순서) 와 같은 순서", () => {
  // depth 차가 0.01 인 소품 사이에 순번이 10 넘게 벌어져도 뒤집히지 않아야 한다
  expect(zIndexOf(17.3, 50)).toBeLessThan(zIndexOf(17.31, 5));
  expect(zIndexOf(23, 3)).toBeLessThan(zIndexOf(23, 4));
  for (const kind of KINDS) {
    const world = fp.layouts[kind].placements
      .map((pl, seq) => ({ pl, seq }))
      .filter(({ pl }) => pl.layer === "world" || pl.layer === "fx");
    const byZ = [...world].sort(
      (a, b) => zIndexOf(a.pl.depth, a.seq) - zIndexOf(b.pl.depth, b.seq),
    );
    const byTuple = [...world].sort((a, b) => a.pl.depth - b.pl.depth || a.seq - b.seq);
    expect(byZ.map((w) => w.pl.id)).toEqual(byTuple.map((w) => w.pl.id));
  }
});

test("themed 소품은 이름 끝 테마를 바꾸고, 바뀐 이름이 시트에 있다", () => {
  const sofa = fp.layouts.large.placements.find((p) => p.id === "sofa")!;
  expect(spriteName(sofa, "codex")).toBe("sofa_codex");
  const seat = fp.layouts.small.placements.find((p) => p.id === "review_seat")!;
  expect(spriteName(seat, "gemini")).toBe("review_sofa_seat_gemini");
  const desk = fp.layouts.large.placements.find((p) => p.id === "dev_desk_0")!;
  expect(spriteName(desk, "codex")).toBe(desk.sprite);
  const frames = office.frames as Record<string, unknown>;
  for (const kind of KINDS)
    for (const theme of THEMES)
      for (const pl of fp.layouts[kind].placements)
        expect(
          frames[`${spriteName(pl, theme)}/${pl.state}`],
          `${kind} ${theme} ${pl.id}`,
        ).toBeDefined();
});

test("좌석: 개발실 6 + 임시 4, 대기업 팀장 1, 중소기업 팀장석 4 + 간이 2", () => {
  const count = (k: Kind) =>
    Object.fromEntries(Object.entries(fp.layouts[k].seats).map(([g, s]) => [g, s.length]));
  expect(count("large")).toEqual({ dev: 6, temp: 4, boss: 1, spare: 0 });
  expect(count("small")).toEqual({ dev: 6, temp: 4, boss: 4, spare: 2 });
});

test("바닥 타일 규칙: 팀장실은 테마 카펫, 장애 상황실은 장애 때만 켜짐, 체크무늬", () => {
  const [bx, by] = fp.rooms.boss.rect;
  expect(tileFrame(fp, bx, by, "codex", false)).toBe(
    `tile/boss_codex/${(bx + by) % 2 ? "b" : "a"}`,
  );
  const [ix, iy] = fp.rooms.incident.rect;
  expect(tileFrame(fp, ix, iy, "claude", true)).toMatch(/^tile\/incident_on\//);
  expect(tileFrame(fp, ix, iy, "claude", false)).toMatch(/^tile\/incident_off\//);
  expect(tileFrame(fp, 0, 0, "claude", false)).toBe("tile/library/a");
  expect(tileFrame(fp, 1, 0, "claude", false)).toBe("tile/library/b");
});

test("렌더러 상수와 다른 평면도는 거부한다", () => {
  expect(() => parseFloorplan({ ...rawFloorplan, canvas1x: { w: 640, h: 320 } })).toThrow();
  expect(() => parseFloorplan(null)).toThrow();
  expect(() => parseFloorplan({ ...rawFloorplan, rooms: null })).toThrow();
  expect(() => parseFloorplan({ ...rawFloorplan, themes: null })).toThrow();
});

test("14개 방은 바닥 전체를 한 번씩 덮고 새 방 자리는 자기 방 안에 있다", () => {
  expect(Object.keys(fp.rooms)).toHaveLength(14);
  for (let x = 0; x < 24; x++)
    for (let y = 0; y < 16; y++) {
      expect(
        Object.values(fp.rooms).filter(
          ({ rect: [a, b, c, d] }) => a <= x && x < c && b <= y && y < d,
        ),
      ).toHaveLength(1);
    }
  for (const key of ["archive", "conference", "security", "roof"])
    for (const [x, y] of fp.stands[key]) expect(roomAt(fp, x, y)).toBe(key);
});
