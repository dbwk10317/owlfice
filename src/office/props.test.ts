// @vitest-environment jsdom
import { Sprite, Texture, type Spritesheet } from "pixi.js";
import { expect, test, vi } from "vite-plus/test";
import rawFloorplan from "../../design/sprites/floorplan.json";
import officeJson from "../../design/sprites/office.json";
import type { Light, OfficeDto, Pose, Room, TeamDto, TodoItem } from "../types/ipc.ts";
import { parseFloorplan, type Kind } from "./floorplan.ts";
import {
  kanbanNotes,
  paperCount,
  plantState,
  PropsLayer,
  safeMinutes,
  trashState,
  windowState,
} from "./props.ts";
import { buildScene } from "./renderer.ts";

// jsdom 에는 canvas 가 없어 파티션 색 굽기는 건너뛴다 (소품과 무관)
vi.mock("./sprites.ts", async (orig) => ({
  ...(await orig<typeof import("./sprites.ts")>()),
  tintedTexture: () => Texture.EMPTY,
}));

const fp = parseFloorplan(rawFloorplan);
// 프레임마다 다른 텍스처라 어떤 프레임이 붙었는지 비교할 수 있다
const textures = Object.fromEntries(
  Object.keys(officeJson.frames).map((k) => [k, new Texture({})]),
) as Record<string, Texture>;
const sheet = { data: officeJson, textures } as unknown as Spritesheet;
const assets = { sheet, floorplan: fp };
const frameOf = (s: Sprite) => Object.keys(textures).find((k) => textures[k] === s.texture);

const NOW = new Date(2026, 9, 21, 12, 0).getTime();
const at = (hour: number) => new Date(2026, 9, 21, hour, 0).getTime();
const ago = (min: number) => new Date(NOW - min * 60_000).toISOString();

function team(deskSlot: number | null, plant: number | null): TeamDto {
  return { teamId: `t${deskSlot}`, deskSlot, props: { plant } } as unknown as TeamDto;
}

function office(shared: Partial<OfficeDto["shared"]> = {}, teams: TeamDto[] = []): OfficeDto {
  return {
    teams,
    shared: {
      light: "Off" as Light,
      safeSince: ago(0),
      trash: 0,
      pizza: 0,
      kanban: [],
      night: false,
      ...shared,
    },
  } as unknown as OfficeDto;
}

function layer(kind: Kind = "large") {
  const scene = buildScene(assets, kind, "claude", false);
  return { scene, props: new PropsLayer(fp, sheet, kind, scene) };
}

const withFrame = (children: unknown[], frame: string) =>
  (children as Sprite[]).filter((c) => c.texture === textures[frame]);

const todo = (status: TodoItem["status"], n = 1): TodoItem[] =>
  Array.from({ length: n }, () => ({ content: "x", status }));

test("신호등: Light → 프레임", () => {
  const { scene, props } = layer();
  props.update(office({ light: "Green" }), NOW);
  expect(frameOf(scene.driven.get("ci_light")!)).toBe("ci_light/green");
  props.update(office({ light: "Red" }), NOW);
  expect(frameOf(scene.driven.get("ci_light")!)).toBe("ci_light/red");
});

test("화분: 20% 단위 경계, 1 초과는 시듦, null 은 기본", () => {
  expect([0, 0.2, 0.21, 0.4, 0.6, 0.8, 0.81, 1, 1.01].map(plantState)).toEqual([
    "s0",
    "s0",
    "s1",
    "s1",
    "s2",
    "s3",
    "s4",
    "s4",
    "wilted",
  ]);
  expect(plantState(null)).toBeUndefined();
  const { scene, props } = layer();
  const plant = scene.driven.get("plant_boss_0")!;
  props.update(office({}, [team(0, 0.9)]), NOW);
  expect(frameOf(plant)).toBe("plant/s4");
  // 단계마다 기준점이 달라 pivot 도 따라가야 발이 제자리에 있다
  expect([plant.pivot.x, plant.pivot.y]).toEqual(officeJson.frames["plant/s4"].anchorPx);
  props.update(office({}, [team(0, null)]), NOW);
  expect(frameOf(plant)).toBe("plant/s0");
});

test("중소기업 화분은 팀장석마다 그 팀 값, 빈 자리는 숨김", () => {
  const { scene, props } = layer("small");
  const p = (i: number) => scene.driven.get(`plant_boss_${i}`)!;
  props.update(office({}, [team(1, 0.5), team(3, 1.5)]), NOW);
  expect([0, 1, 2, 3].map((i) => p(i).visible)).toEqual([false, true, false, true]);
  expect(frameOf(p(1))).toBe("plant/s2");
  expect(frameOf(p(3))).toBe("plant/wilted");
  props.update(null, NOW);
  expect([0, 1, 2, 3].map((i) => p(i).visible)).toEqual([false, false, false, false]);
});

test("전광판: 분 계산 — 0, 9999, 넘침, 거꾸로 간 시계, 읽을 수 없는 시각", () => {
  expect(safeMinutes(ago(0), NOW)).toBe(0);
  expect(safeMinutes(ago(37.9), NOW)).toBe(37);
  expect(safeMinutes(ago(9999), NOW)).toBe(9999);
  expect(safeMinutes(ago(10_000), NOW)).toBe(9999);
  expect(safeMinutes(ago(-5), NOW)).toBe(0);
  expect(safeMinutes("??", NOW)).toBe(0);
});

test("전광판 숫자는 digitOrigin 에서 digitStep 씩, 4자리까지", () => {
  const { scene, props } = layer();
  const digits = () =>
    (scene.world.children as Sprite[])
      .map((c) => [frameOf(c)!, c.x, c.y] as const)
      .filter(([f]) => f?.startsWith("digit/"))
      .sort((a, b) => a[1] - b[1]);
  const board = fp.layouts.large.placements.find((p) => p.id === "safety_board")!;
  const { digitOrigin: o, digitStep: d } = officeJson.owlfice.props.safety_board;

  props.update(office({ safeSince: ago(0) }), NOW);
  expect(digits().map(([f]) => f)).toEqual(["digit/0"]);

  props.update(office({ safeSince: ago(1234) }), NOW);
  expect(digits()).toEqual(
    ["1", "2", "3", "4"].map((c, n) => [
      `digit/${c}`,
      board.x + o[0] + n * d[0],
      board.y + o[1] + n * d[1],
    ]),
  );

  props.update(office({ safeSince: ago(123_456) }), NOW);
  expect(digits().map(([f]) => f)).toEqual(Array(4).fill("digit/9"));

  props.update(office({ safeSince: ago(1234) }), NOW + 60_000);
  expect(digits().map(([f]) => f)).toEqual(["digit/1", "digit/2", "digit/3", "digit/5"]);
});

test("휴지통: 사고 수 → 상태, 10개 넘으면 종이가 하나씩", () => {
  expect([0, 1, 4, 5, 99].map(trashState)).toEqual(["empty", "some", "some", "full", "full"]);
  expect([0, 10, 11, 20, 21, 99].map((n) => paperCount(n, 10))).toEqual([0, 0, 1, 10, 10, 10]);
  const { scene, props } = layer();
  const papers = () => withFrame(scene.world.children, "paper_ball/default");
  props.update(office({ trash: 5 }), NOW);
  expect(frameOf(scene.driven.get("trash")!)).toBe("trash/full");
  expect(papers()).toHaveLength(0);
  props.update(office({ trash: 13 }), NOW);
  expect(papers()).toHaveLength(3);
  props.update(office({ trash: 2 }), NOW);
  expect(papers()).toHaveLength(0);
  expect(frameOf(scene.driven.get("trash")!)).toBe("trash/some");
});

test("칸반: 열 우선 슬롯, 열이 넘치면 자른다", () => {
  expect(kanbanNotes([...todo("Pending", 2), ...todo("InProgress"), ...todo("Done", 2)])).toEqual([
    { slot: 0, color: "yellow" },
    { slot: 1, color: "pink" },
    { slot: 3, color: "blue" },
    { slot: 6, color: "green" },
    { slot: 7, color: "green" },
  ]);
  expect(kanbanNotes([...todo("Pending", 5), ...todo("Done", 5)]).map((n) => n.slot)).toEqual([
    0, 1, 2, 6, 7, 8,
  ]);

  const { scene, props } = layer();
  const kb = fp.layouts.large.placements.find((p) => p.id === "kanban")!;
  const slots = officeJson.owlfice.props.kanban_l.slots;
  props.update(office({ kanban: [...todo("InProgress"), ...todo("Done")] }), NOW);
  const [blue] = withFrame(scene.wall.children, "wall_note/blue");
  expect([blue.x, blue.y]).toEqual([kb.x + slots[3][0], kb.y + slots[3][1]]);
  const [green] = withFrame(scene.wall.children, "wall_note/green");
  expect([green.x, green.y]).toEqual([kb.x + slots[6][0], kb.y + slots[6][1]]);
  props.update(office(), NOW);
  expect(withFrame(scene.wall.children, "wall_note/blue")).toHaveLength(0);
});

test("피자: 개수만큼 쌓고 8개에서 멈춘다", () => {
  const { scene, props } = layer();
  const boxes = () => withFrame(scene.world.children, "pizza_box/default");
  props.update(office({ pizza: 3 }), NOW);
  const ys = boxes()
    .map((b) => b.y)
    .sort((a, b) => b - a);
  expect(ys).toHaveLength(3);
  // 상자마다 z 가 2 올라 화면에서 2px 위
  expect([ys[0] - ys[1], ys[1] - ys[2]]).toEqual([2, 2]);
  props.update(office({ pizza: 20 }), NOW);
  expect(boxes()).toHaveLength(8);
});

test("창밖: 06·17·22시 경계, 컵라면은 야근일 때만", () => {
  expect([5, 6, 16, 17, 21, 22, 23, 0].map(windowState)).toEqual([
    "night",
    "day",
    "day",
    "dusk",
    "dusk",
    "night",
    "night",
    "night",
  ]);
  const { scene, props } = layer();
  const win = scene.driven.get("window")!;
  props.update(office(), at(17));
  expect(frameOf(win)).toBe("window_l/dusk");
  props.update(office({ night: false }), at(22));
  expect(frameOf(win)).toBe("window_l/night");
  expect(scene.driven.get("ramen")!.visible).toBe(false);
  props.update(office({ night: true }), at(22));
  expect(scene.driven.get("ramen")!.visible).toBe(true);
  // 중소기업 창은 xl
  const small = layer("small");
  small.props.update(office(), at(9));
  expect(frameOf(small.scene.driven.get("window")!)).toBe("window_xl/day");
});

test("TV: 값이 있는 팀 수만큼 점, 같은 값이면 그대로", () => {
  const { scene, props } = layer();
  const count = () => scene.wall.children.length;
  const base = count();
  props.update(office({}, [team(0, 0.5), team(1, null), team(2, 2)]), NOW);
  expect(count()).toBe(base + 1);
  const g = scene.wall.children[base];
  props.update(office({}, [team(0, 0.5), team(1, null), team(2, 2)]), NOW);
  expect(scene.wall.children[base]).toBe(g);
  props.update(null, NOW);
  expect(count()).toBe(base + 1);
});

test("같은 값이면 스프라이트를 다시 만들지 않고, 바뀐 소품만 새로 만든다", () => {
  const { scene, props } = layer();
  const o = (pizza: number) => office({ trash: 14, pizza, kanban: todo("Done") }, [team(0, 0.3)]);
  props.update(o(2), NOW);
  const world = [...scene.world.children];
  const wall = [...scene.wall.children];
  props.update(o(2), NOW + 1000);
  expect(scene.world.children.every((c, i) => c === world[i])).toBe(true);

  props.update(o(3), NOW);
  // 피자 2개가 지워지고 3개가 새로 생긴다. 종이·전광판·월드의 나머지는 그대로
  const kept = scene.world.children.filter((c) => world.includes(c));
  expect(kept).toHaveLength(world.length - 2);
  expect(
    withFrame(scene.world.children, "paper_ball/default").every((c) => world.includes(c)),
  ).toBe(true);
  expect(scene.wall.children).toEqual(wall);
});

// ---------- 부장 책상 소품 (응답 대기·러버덕·결재 대기·인터폰) ----------

function bossTeam(deskSlot: number | null, room: Room, pose: Pose): TeamDto {
  return {
    teamId: `t${deskSlot}`,
    deskSlot,
    props: { plant: null },
    employees: [{ activity: { room, pose } }],
  } as unknown as TeamDto;
}

test("응답 대기 김, 러버덕 집어 듦, 결재 대기 화면보호기는 부장 자세를 따라간다", () => {
  const { scene, props } = layer();
  const mug = scene.driven.get("mug_0")!;
  const duck = scene.driven.get("duck_0")!;
  const mons = ["boss_mon_0_0", "boss_mon_0_1"].map((id) => scene.driven.get(id)!);
  const at = (pose: Pose, room: Room = "TeamLead") => {
    props.update(office({}, [bossTeam(0, room, pose)]), NOW);
  };
  at("Sit");
  expect(frameOf(mug)).toBe("mug/still");
  expect(duck.visible).toBe(true);
  expect(mons.map(frameOf)).toEqual(["monitor_lg/off", "monitor_lg/off"]);

  at("Think");
  expect(frameOf(mug)).toMatch(/^mug\/steam_[01]$/);
  // 김은 두 프레임이 번갈아 오른다
  props.tick(NOW + 334);
  const a = frameOf(mug);
  props.tick(NOW + 667);
  expect(frameOf(mug)).not.toBe(a);

  at("Duck");
  expect(duck.visible).toBe(false);
  expect(frameOf(mug)).toBe("mug/still");

  at("Pace", "Dev");
  expect(mons.map(frameOf)).toEqual([
    expect.stringMatching(/^monitor_lg\/saver_[01]$/),
    expect.stringMatching(/^monitor_lg\/saver_[01]$/),
  ]);
  expect(duck.visible).toBe(true);

  // 부장이 다른 방에서 생각하면 책상 앞 김은 없다
  at("Think", "Dev");
  expect(frameOf(mug)).toBe("mug/still");
  props.update(null, NOW);
  expect(mons.map(frameOf)).toEqual(["monitor_lg/off", "monitor_lg/off"]);
});

test("중소기업은 팀장석마다 그 팀 부장 모니터만 화면보호기, 김·러버덕 소품은 없다", () => {
  const { scene, props } = layer("small");
  props.update(office({}, [bossTeam(2, "Dev", "Pace"), bossTeam(0, "TeamLead", "Sit")]), NOW);
  const f = (i: number) => frameOf(scene.driven.get(`boss_mon_${i}_0`)!);
  expect([0, 1, 2, 3].map(f)).toEqual([
    "monitor_md/off",
    "monitor_md/off",
    expect.stringMatching(/^monitor_md\/saver_[01]$/),
    "monitor_md/off",
  ]);
  expect(scene.driven.has("mug_0")).toBe(false);
});

test("인터폰은 ring 뒤 1초만 울린다", () => {
  const { scene, props } = layer();
  const bell = scene.driven.get("interphone")!;
  expect(frameOf(bell)).toBe("interphone/idle");
  props.ring(NOW);
  expect(frameOf(bell)).toMatch(/^interphone\/ring_[01]$/);
  props.tick(NOW + 999);
  expect(frameOf(bell)).toMatch(/^interphone\/ring_[01]$/);
  props.tick(NOW + 1000);
  expect(frameOf(bell)).toBe("interphone/idle");
});

test("퇴근한 팀 부장의 책상 소품은 모두 꺼진다", () => {
  const { scene, props } = layer();
  const off = { ...bossTeam(0, "TeamLead", "Think"), phase: "Off" } as TeamDto;
  props.update(office({}, [off]), NOW);
  expect(frameOf(scene.driven.get("mug_0")!)).toBe("mug/still");
  const pace = { ...bossTeam(0, "TeamLead", "Pace"), phase: "Off" } as TeamDto;
  props.update(office({}, [pace]), NOW);
  expect(frameOf(scene.driven.get("boss_mon_0_0")!)).toBe("monitor_lg/off");
  const duck = { ...bossTeam(0, "TeamLead", "Duck"), phase: "Off" } as TeamDto;
  props.update(office({}, [duck]), NOW);
  expect(scene.driven.get("duck_0")!.visible).toBe(true);
});
