// @vitest-environment jsdom
import { Sprite, Texture, type Spritesheet } from "pixi.js";
import { expect, test, vi } from "vite-plus/test";
import rawFloorplan from "../../design/sprites/floorplan.json";
import officeJson from "../../design/sprites/office.json";
import type { Light, OfficeDto, Pose, Room, TeamDto, TodoItem } from "../types/ipc.ts";
import { parseFloorplan, type Kind } from "./floorplan.ts";
import {
  DOOR_OPEN_MS,
  FIREWORKS_MS,
  LADDER_MS,
  REPOT_MS,
  RETURN_MS,
  STAMP_RESULT_MS,
  TRUCK_MS,
} from "./anim-queue.ts";
import {
  kanbanNotes,
  paperCount,
  plantState,
  PropsLayer,
  ScreenLabel,
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
  return {
    teamId: `t${deskSlot}`,
    deskSlot,
    props: { plant },
    employees: [],
  } as unknown as TeamDto;
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

// ---------- 빌드실·테스트실·출고장 (T25) ----------

const SINCE = "2026-10-21T10:00:00Z";
function emp(room: Room, pose: Pose = "Sit", agentId = "e"): TeamDto["employees"][number] {
  return { agentId, activity: { room, pose, since: SINCE } } as TeamDto["employees"][number];
}
function crew(...employees: TeamDto["employees"]): OfficeDto {
  const t = { ...team(0, null), phase: "Working", employees } as TeamDto;
  return office({}, [t]);
}
type Built = ReturnType<typeof layer>["scene"];
const tex = (scene: Built, id: string) => frameOf(scene.driven.get(id)!);
const sprites = (scene: Built, frame: string) => withFrame(scene.world.children, frame) as Sprite[];
const count = (scene: Built, frame: string) => sprites(scene, frame).length;
const stillMotion = (on: boolean, fn: () => void) => {
  vi.stubGlobal("matchMedia", () => ({ matches: on }));
  try {
    fn();
  } finally {
    vi.unstubAllGlobals();
  }
};

test("셸 명령 중: 랙 LED 가 깜빡이고 단말기가 켜진다. 사람이 없으면 평면도 기본", () => {
  for (const kind of ["large", "small"] as const) {
    const { scene, props } = layer(kind);
    props.update(crew(emp("Build")), NOW);
    expect(tex(scene, "terminal")).toBe("terminal/on");
    const leds = [0, 200, 400].map((ms) => {
      props.tick(NOW + ms);
      return [tex(scene, "rack_0"), tex(scene, "rack_1")];
    });
    expect(new Set(leds.flat())).toEqual(new Set(["rack/led_0", "rack/led_1"]));
    expect(tex(scene, "fan")).toBe("fan/off");
    props.update(crew(emp("Dev")), NOW + 1000);
    expect([tex(scene, "terminal"), tex(scene, "rack_0"), tex(scene, "rack_1")]).toEqual([
      "terminal/off",
      "rack/led_0",
      "rack/led_0",
    ]);
  }
});

test("명령이 평균 2배를 넘으면(쪼그려 앉음) 선풍기가 돈다", () => {
  const { scene, props } = layer();
  props.update(crew(emp("Build", "Squat")), NOW);
  const fans = [0, 50, 100].map((ms) => {
    props.tick(NOW + ms);
    return tex(scene, "fan");
  });
  expect(new Set(fans)).toEqual(new Set(["fan/spin_0", "fan/spin_1"]));
  props.update(crew(emp("Build")), NOW + 500);
  expect(tex(scene, "fan")).toBe("fan/off");
  // 테스트실에서 쪼그린 직원은 빌드실 선풍기를 돌리지 않는다
  props.update(crew(emp("Test", "Squat")), NOW + 600);
  expect(tex(scene, "fan")).toBe("fan/off");
});

test("테스트 중에는 테스트 기계가 돈다", () => {
  const { scene, props } = layer();
  props.update(crew(emp("Test")), NOW);
  const frames = [0, 100, 200].map((ms) => {
    props.tick(NOW + ms);
    return tex(scene, "test_machine");
  });
  expect(new Set(frames)).toEqual(new Set(["test_machine/run_0", "test_machine/run_1"]));
  props.update(crew(emp("Dev")), NOW + 500);
  expect(tex(scene, "test_machine")).toBe("test_machine/idle");
});

test("퇴근한 팀과 null 은 소품을 켜지 않는다", () => {
  const { scene, props } = layer();
  const off = crew(emp("Build"), emp("Test", "Sit", "f"), emp("Shipping", "Sit", "g"));
  off.teams[0].phase = "Off";
  props.update(off, NOW);
  props.update(null, NOW);
  expect([tex(scene, "terminal"), tex(scene, "test_machine")]).toEqual([
    "terminal/off",
    "test_machine/idle",
  ]);
  expect(count(scene, "box_taped/default")).toBe(0);
});

test("움직임 줄이기: 반복 프레임은 첫 장에 멈춘다", () =>
  stillMotion(true, () => {
    const { scene, props } = layer();
    props.update(crew(emp("Build", "Squat"), emp("Test", "Sit", "t")), NOW);
    for (const ms of [0, 200, 400]) {
      props.tick(NOW + ms);
      expect([tex(scene, "rack_0"), tex(scene, "fan"), tex(scene, "test_machine")]).toEqual([
        "rack/led_0",
        "fan/spin_0",
        "test_machine/run_0",
      ]);
    }
  }));

test("출고장에 있는 직원마다 포장된 상자가 하나씩, 슬롯 수까지", () => {
  const { scene, props } = layer();
  const n = (k: number) =>
    crew(...Array.from({ length: k }, (_, i) => emp("Shipping", "Sit", `e${i}`)));
  const boxes = (o: OfficeDto) => {
    props.update(o, NOW);
    return count(scene, "box_taped/default");
  };
  expect([0, 1, 2, 3, 5].map((k) => boxes(n(k)))).toEqual([0, 1, 2, 3, 3]);
  // 같은 수로 다시 그려도 상자를 늘리지 않는다
  expect(boxes(n(3))).toBe(3);
  expect(boxes(n(0))).toBe(0);
});

test("백그라운드 명령: Paper cue 가 작업표를 꽂고 빼고, 장면을 다시 지어도 남는다", () => {
  const { scene, props } = layer();
  const ticket = () => scene.driven.get("job_ticket")!.visible;
  expect(ticket()).toBe(false);
  props.cue({ teamId: "t", kind: "Paper", agent: "a", on: true }, NOW);
  props.cue({ teamId: "t", kind: "Paper", agent: "b", on: true }, NOW);
  expect(ticket()).toBe(true);
  props.cue({ teamId: "t", kind: "Paper", agent: "a", on: false }, NOW);
  expect(ticket()).toBe(true);
  // 장애로 장면을 다시 지어도 (renderer 가 memory 를 넘긴다) 작업표가 남는다
  const next = buildScene(assets, "large", "claude", true);
  const again = new PropsLayer(fp, sheet, "large", next, props.memory);
  again.update(office(), NOW + 1000);
  expect(next.driven.get("job_ticket")!.visible).toBe(true);
  props.cue({ teamId: "t", kind: "Paper", agent: "b", on: false }, NOW);
  again.update(office(), NOW + 2000);
  expect(next.driven.get("job_ticket")!.visible).toBe(false);
});

test("reset: 사무실을 옮기면 작업표·테스트 결과·진행 중인 효과가 사라진다", () => {
  const { scene, props } = layer();
  props.cue({ teamId: "t", kind: "Paper", agent: "a", on: true }, NOW);
  props.cue({ teamId: "t", kind: "Stamp", passed: 1, failed: 1, estimated: false }, NOW);
  props.cue({ teamId: "t", kind: "Truck" }, NOW);
  props.reset();
  props.update(office(), NOW + 10);
  expect(scene.driven.get("job_ticket")!.visible).toBe(false);
  expect(tex(scene, "test_board")).toBe("test_board/pass");
  expect(count(scene, "stamp_fail/default") + count(scene, "truck/drive_0")).toBe(0);
});

const stampCue = (passed: number, failed: number, estimated = false) =>
  ({ teamId: "t", kind: "Stamp", passed, failed, estimated }) as const;

test("Stamp: 전부 통과 / 일부 실패 / 추정 각각의 도장과 결과판, 2초 뒤 도장만 사라진다", () => {
  const cases: [number, number, boolean, string, string][] = [
    [12, 0, false, "stamp_pass/default", "test_board/pass"],
    [10, 2, false, "stamp_fail/default", "test_board/fail"],
    [12, 0, true, "stamp_pass_est/default", "test_board/pass"],
    [10, 2, true, "stamp_fail_est/default", "test_board/fail"],
  ];
  for (const [passed, failed, estimated, mark, board] of cases) {
    const { scene, props } = layer();
    props.cue(stampCue(passed, failed, estimated), NOW);
    expect(count(scene, mark)).toBe(1);
    expect(tex(scene, "test_board")).toBe(board);
    props.tick(NOW + STAMP_RESULT_MS - 1);
    expect(count(scene, mark)).toBe(1);
    props.tick(NOW + STAMP_RESULT_MS);
    expect(count(scene, mark)).toBe(0);
    // 결과판은 다음 테스트까지 남고, 다시 그려도 유지된다
    props.update(office(), NOW + STAMP_RESULT_MS + 1000);
    expect(tex(scene, "test_board")).toBe(board);
  }
});

test("Stamp 가 연달아 오면 앞 도장을 치우고 새것 하나만 둔다", () => {
  const { scene, props } = layer();
  props.cue(stampCue(1, 1), NOW);
  props.cue(stampCue(2, 0), NOW + 500);
  expect(count(scene, "stamp_fail/default")).toBe(0);
  expect(count(scene, "stamp_pass/default")).toBe(1);
});

test("Truck: 출고장 앞에서 출발해 +gy 쪽으로 가고 TRUCK_MS 에 사라진다", () => {
  const { scene, props } = layer();
  props.cue({ teamId: "t", kind: "Truck" }, NOW);
  const truck = () =>
    (scene.world.children as Sprite[]).find((c) => frameOf(c)?.startsWith("truck/"));
  const start = { x: truck()!.position.x, y: truck()!.position.y };
  props.tick(NOW + TRUCK_MS / 2);
  // +gy 로 가면 화면은 왼쪽 아래
  expect(truck()!.position.x).toBeLessThan(start.x);
  expect(truck()!.position.y).toBeGreaterThan(start.y);
  props.tick(NOW + TRUCK_MS - 1);
  expect(truck()).toBeDefined();
  props.tick(NOW + TRUCK_MS);
  expect(truck()).toBeUndefined();
  // 움직임 줄이기: 제자리에서 같은 시간 동안만 보인다
  stillMotion(true, () => {
    const r = layer();
    r.props.cue({ teamId: "t", kind: "Truck" }, NOW);
    const t = () => sprites(r.scene, "truck/drive_0");
    const p0 = { x: t()[0].position.x, y: t()[0].position.y };
    r.props.tick(NOW + TRUCK_MS - 1);
    expect([t()[0].position.x, t()[0].position.y]).toEqual([p0.x, p0.y]);
    r.props.tick(NOW + TRUCK_MS);
    expect(t()).toHaveLength(0);
  });
});

test("Return: 상자가 떨어져 바닥에 닿고 반송 쪽지와 함께 RETURN_MS 뒤 사라진다", () => {
  const { scene, props } = layer();
  props.cue({ teamId: "t", kind: "Return" }, NOW);
  const box = () => sprites(scene, "box_taped/default")[0];
  const note = () => sprites(scene, "note_return/default")[0];
  const high = box().position.y;
  expect(note()).toBeDefined();
  props.tick(NOW + 1000);
  const floor = box().position.y;
  expect(floor).toBeGreaterThan(high);
  // 쪽지는 상자 위에 붙어 다닌다
  expect(note().position.y).toBeLessThan(floor);
  props.tick(NOW + RETURN_MS - 1);
  expect(note()).toBeDefined();
  props.tick(NOW + RETURN_MS);
  expect([box(), note()]).toEqual([undefined, undefined]);
  // 움직임 줄이기: 처음부터 바닥
  stillMotion(true, () => {
    const r = layer();
    r.props.cue({ teamId: "t", kind: "Return" }, NOW);
    expect(sprites(r.scene, "box_taped/default")[0].position.y).toBe(floor);
  });
});

test("Fireworks: 0→1→2 한 번, FIREWORKS_MS 에 사라진다", () => {
  const { scene, props } = layer();
  props.cue({ teamId: "t", kind: "Fireworks" }, NOW);
  const shown = (t: number) => {
    props.tick(NOW + t);
    return [0, 1, 2].find((i) => count(scene, `fireworks/${i}`) === 1);
  };
  expect([0, 170, 340, FIREWORKS_MS - 1].map(shown)).toEqual([0, 1, 2, 2]);
  props.tick(NOW + FIREWORKS_MS);
  expect([0, 1, 2].map((i) => count(scene, `fireworks/${i}`))).toEqual([0, 0, 0]);
  // 움직임 줄이기: 마지막 프레임만 같은 시간 동안
  stillMotion(true, () => {
    const r = layer();
    r.props.cue({ teamId: "t", kind: "Fireworks" }, NOW);
    r.props.tick(NOW + FIREWORKS_MS - 1);
    expect([0, 1, 2].map((i) => count(r.scene, `fireworks/${i}`))).toEqual([0, 0, 1]);
    r.props.tick(NOW + FIREWORKS_MS);
    expect([0, 1, 2].map((i) => count(r.scene, `fireworks/${i}`))).toEqual([0, 0, 0]);
  });
});

// ---------- 미팅실·외근·장애·리뷰실 (T26) ----------

function says(room: Room, sentence: string, o: object = {}): TeamDto["employees"][number] {
  return {
    agentId: "e",
    retiredAt: null,
    activity: { room, pose: "Sit", since: SINCE, sentence },
    ...o,
  } as TeamDto["employees"][number];
}
const MCP = "거래처 `figma`와 `get_file` 건으로 미팅 중";
const SKILL = "외주 매뉴얼 `pdf` 펼쳐보는 중";
const review = (n: number) => `PR 리뷰 중 · 빨간펜 ${n}개`;

test("MCP 미팅: 화면 켜짐, 명함, 상호 글자(DOM). 스킬은 매뉴얼. 끝나면 기본", () => {
  const overlay = document.createElement("div");
  const label = new ScreenLabel(overlay);
  const scene = buildScene(assets, "large", "claude", false);
  const props = new PropsLayer(fp, sheet, "large", scene, undefined, label);
  const shown = () => (overlay.firstElementChild as HTMLElement).hidden;

  props.update(crew(says("Meeting", MCP)), NOW);
  expect(tex(scene, "meeting_screen")).toBe("meeting_screen/on");
  expect([count(scene, "business_card/default"), count(scene, "manual_open/default")]).toEqual([
    1, 0,
  ]);
  expect(overlay.textContent).toBe("figma");
  expect(shown()).toBe(false);

  props.update(crew(says("Meeting", SKILL)), NOW);
  expect(tex(scene, "meeting_screen")).toBe("meeting_screen/off");
  expect([count(scene, "business_card/default"), count(scene, "manual_open/default")]).toEqual([
    0, 1,
  ]);
  expect(shown()).toBe(true);

  props.update(crew(says("Dev", MCP)), NOW);
  expect(count(scene, "manual_open/default")).toBe(0);
  props.update(null, NOW);
  expect(tex(scene, "meeting_screen")).toBe("meeting_screen/off");
});

test("상호는 textContent 로만 넣는다", () => {
  const overlay = document.createElement("div");
  const scene = buildScene(assets, "large", "claude", false);
  const props = new PropsLayer(fp, sheet, "large", scene, undefined, new ScreenLabel(overlay));
  props.update(crew(says("Meeting", "거래처 `<img src=x>`와 `t` 건으로 미팅 중")), NOW);
  expect(overlay.querySelector("img")).toBeNull();
  expect(overlay.textContent).toBe("<img src=x>");
});

test("외근 중인 직원 자리 책상 위에 팻말, 돌아오거나 퇴근·퇴장이면 걷는다", () => {
  for (const kind of ["large", "small"] as const) {
    const { scene, props } = layer(kind);
    const out = (sub: object = {}) =>
      crew(says("Outside", "외근 중", { agentId: "b" }), says("Outside", "외근 중", sub));
    props.update(out({ seat: 2 }), NOW);
    expect(count(scene, "sign_away/default")).toBe(2);
    // 책상 하나에 하나씩, 직원 자리가 다르면 위치도 다르다
    const [a, b] = sprites(scene, "sign_away/default");
    expect([a.x, a.y]).not.toEqual([b.x, b.y]);
    props.update(out({ seat: 2, retiredAt: SINCE }), NOW);
    expect(count(scene, "sign_away/default")).toBe(1);
    props.update(crew(says("Dev", "일하는 중", { seat: 2 })), NOW);
    expect(count(scene, "sign_away/default")).toBe(0);
    const off = out({ seat: 2 });
    off.teams[0].phase = "Off";
    props.update(off, NOW);
    expect(count(scene, "sign_away/default")).toBe(0);
  }
});

test("리뷰실: 직원이 있으면 화면 code, 빨간펜 수만큼 포스트잇(화면 슬롯 6개까지)", () => {
  const { scene, props } = layer();
  const pens = () => count(scene, "postit_red/default");
  props.update(crew(says("Review", review(0))), NOW);
  expect([tex(scene, "review_screen"), pens()]).toEqual(["review_screen/code", 0]);
  props.update(crew(says("Review", review(3))), NOW);
  expect(pens()).toBe(3);
  const rs = fp.layouts.large.placements.find((p) => p.id === "review_screen")!;
  const slots = officeJson.owlfice.props.review_screen.slots;
  const at = (xy: number[]) => xy.join();
  expect(new Set(sprites(scene, "postit_red/default").map((s) => at([s.x, s.y])))).toEqual(
    new Set(slots.slice(0, 3).map(([dx, dy]) => at([rs.x + dx, rs.y + dy]))),
  );
  props.update(crew(says("Review", review(9))), NOW);
  expect(pens()).toBe(slots.length);
  // 두 사람이 리뷰하면 더 많은 쪽
  props.update(crew(says("Review", review(2)), says("Review", review(4), { agentId: "f" })), NOW);
  expect(pens()).toBe(4);
  props.update(crew(says("Dev", review(4))), NOW);
  expect([tex(scene, "review_screen"), pens()]).toEqual(["review_screen/off", 0]);
});

test("장애: 화이트보드 낙서, 탁자 켜짐, 경광등이 돈다. 해제하면 기본. 움직임 줄이기면 경광등 첫 장", () => {
  const { scene, props } = layer();
  const inc = (incident: boolean) => office({ incident }, []);
  props.update(inc(true), NOW);
  expect([tex(scene, "whiteboard"), tex(scene, "incident_table")]).toEqual([
    "whiteboard/scribble",
    "incident_table/on",
  ]);
  const beacon = [0, 80, 160, 240].map((ms) => {
    props.tick(NOW + ms);
    return tex(scene, "beacon");
  });
  expect(new Set(beacon)).toEqual(new Set(["beacon/on_0", "beacon/on_1"]));
  stillMotion(true, () => {
    for (const ms of [0, 80, 160, 240]) {
      props.tick(NOW + ms);
      expect(tex(scene, "beacon")).toBe("beacon/on_0");
    }
  });
  props.update(inc(false), NOW + 1000);
  expect([tex(scene, "whiteboard"), tex(scene, "incident_table"), tex(scene, "beacon")]).toEqual([
    "whiteboard/off",
    "incident_table/off",
    "beacon/off",
  ]);
});

test("외근 복귀: 문이 DOOR_OPEN_MS 동안 열렸다 닫힌다", () => {
  const { scene, props } = layer();
  expect(tex(scene, "door")).toBe("door/closed");
  props.openDoor(NOW);
  expect(tex(scene, "door")).toBe("door/open");
  props.tick(NOW + DOOR_OPEN_MS - 1);
  expect(tex(scene, "door")).toBe("door/open");
  props.tick(NOW + DOOR_OPEN_MS);
  expect(tex(scene, "door")).toBe("door/closed");
});

test("상호 글자: 배율이 바뀌면 left·top·maxWidth 가 따라 바뀐다", () => {
  const overlay = document.createElement("div");
  const label = new ScreenLabel(overlay);
  label.set("figma", 10, 20, 18);
  const el = overlay.firstElementChild as HTMLElement;
  for (const k of [1, 3]) {
    label.place((x, y) => ({ left: x * k, top: y * k }));
    expect([el.style.left, el.style.top, el.style.maxWidth]).toEqual([
      `${10 * k}px`,
      `${20 * k}px`,
      `${18 * k}px`,
    ]);
  }
});

// ---------- 서브 종료·재출근·분갈이·액자 교체 (T26b) ----------

const BOARD = fp.layouts.large.placements.find((p) => p.id === "attendance")!;
const BOARD_SLOTS = officeJson.owlfice.props.attendance.slots as unknown as number[][][];
/** 근태판에 붙은 자석이 어느 칸·줄인지 */
const magnetsOn = (scene: Built) =>
  (scene.wall.children as Sprite[])
    .filter((c) => frameOf(c)?.startsWith("magnet/"))
    .map((c) => {
      const col = BOARD_SLOTS.findIndex((lines) =>
        lines.some(([dx, dy]) => c.x === BOARD.x + dx && c.y === BOARD.y + dy),
      );
      return [frameOf(c)!, col] as const;
    });
const sub = (room: Room, o: object = {}) => says(room, "일하는 중", { agentId: "s", ...o });

test("서브 종료: 탕비실 콘솔에 앉으면 게임기가 켜지고 자석은 퇴근 칸, 재출근하면 둘 다 돌아온다", () => {
  const { scene, props } = layer();
  const lead = says("TeamLead", "일하는 중", { agentId: "boss" });
  props.update(crew(lead, sub("Dev", { seat: 0 })), NOW);
  expect(tex(scene, "game_tv")).toBe("game_tv/off");
  expect(magnetsOn(scene).map(([, col]) => col)).toEqual([0, 0]);
  props.update(crew(lead, sub("Pantry", { retiredAt: SINCE })), NOW);
  props.tick(NOW);
  expect(tex(scene, "game_tv")).toBe("game_tv/game_0");
  props.tick(NOW + 400);
  expect(tex(scene, "game_tv")).toBe("game_tv/game_1");
  expect(magnetsOn(scene).map(([, col]) => col)).toEqual([0, 3]);
  // 재출근: 다시 개발실, retired_at 이 비면 출근 칸
  props.update(crew(lead, sub("Dev", { seat: 0 })), NOW + 1000);
  expect(tex(scene, "game_tv")).toBe("game_tv/off");
  expect(magnetsOn(scene).map(([, col]) => col)).toEqual([0, 0]);
});

test("게임기는 콘솔 자리에 앉은 직원만 켠다: 콘솔을 떠나거나 소파에서 조는 직원은 안 켠다", () => {
  for (const kind of ["large", "small"] as const) {
    const { scene, props } = layer(kind);
    props.update(crew(sub("Pantry", { retiredAt: SINCE, agentId: "x" })), NOW);
    props.tick(NOW);
    expect(tex(scene, "game_tv")).toBe("game_tv/game_0");
    props.update(crew(sub("Dev", { seat: 0, agentId: "x" })), NOW);
    expect(tex(scene, "game_tv")).toBe("game_tv/off");
    // 졸음 부장은 소파라 게임기를 켜지 않는다
    const nap = says("Pantry", "졸음", { agentId: "z" });
    nap.activity.pose = "Sleep";
    props.update(crew(nap), NOW);
    expect(tex(scene, "game_tv")).toBe("game_tv/off");
  }
});

test("근태판: 외근은 외근 칸, 퇴근한 팀은 모두 퇴근 칸, 중소기업은 팀 색으로 팀을 합친다", () => {
  const { scene, props } = layer("small");
  const teamOf = (slot: number, ...e: TeamDto["employees"]) =>
    ({ ...team(slot, null), phase: "Working", employees: e }) as TeamDto;
  const off = { ...teamOf(1, sub("Dev")), phase: "Off" } as TeamDto;
  props.update(
    office({}, [teamOf(0, sub("Dev"), sub("Outside"), sub("Pantry", { retiredAt: SINCE })), off]),
    NOW,
  );
  expect(magnetsOn(scene)).toEqual([
    ["magnet/teal", 0],
    ["magnet/teal", 1],
    ["magnet/teal", 3],
    ["magnet/violet", 3],
  ]);
  props.update(null, NOW);
  expect(magnetsOn(scene)).toEqual([]);
});

test("근태판: 한 칸의 줄 수를 넘으면 뒤 직원은 자석이 없다", () => {
  const { scene, props } = layer();
  const many = Array.from({ length: 7 }, () => sub("Dev"));
  props.update(crew(...many), NOW);
  expect(magnetsOn(scene)).toHaveLength(BOARD_SLOTS[0].length);
});

const plantAt = (scene: Built, id = "plant_boss_0") => scene.driven.get(id)!;

test("Repot: 화분이 REPOT_MS 동안 비었다가 s0 으로 나온다. 팀 하나만, 움직임 줄이기면 결과만", () => {
  const repot = (props: PropsLayer, o: OfficeDto, now: number, teamId = "t0") => {
    // renderer 순서: cue(새 사무실)가 먼저, update 가 그 뒤
    props.cue({ teamId, kind: "Repot" }, now, o);
    props.update(o, now);
  };
  stillMotion(false, () => {
    const { scene, props } = layer("small");
    props.update(office({}, [team(0, 0.9)]), NOW);
    // 새로 생긴 팀 1 은 이 스냅샷에서야 처음 보인다
    repot(props, office({}, [team(0, 0.05), team(1, 0.9)]), NOW + 10);
    expect(plantAt(scene, "plant_boss_0").visible).toBe(false);
    expect(plantAt(scene, "plant_boss_1").visible).toBe(true);
    props.update(office({}, [team(0, 0.05), team(1, 0.9)]), NOW + 500);
    expect(plantAt(scene, "plant_boss_0").visible).toBe(false);
    props.tick(NOW + 10 + REPOT_MS - 1);
    expect(plantAt(scene, "plant_boss_0").visible).toBe(false);
    props.tick(NOW + 10 + REPOT_MS);
    expect(plantAt(scene, "plant_boss_0").visible).toBe(true);
    expect(tex(scene, "plant_boss_0")).toBe("plant/s0");
    // 사무실을 옮기면 진행 중인 분갈이도 없던 일
    props.cue({ teamId: "t0", kind: "Repot" }, NOW + 5000, office({}, [team(0, 0.05)]));
    props.reset();
    props.tick(NOW + 5001);
    expect(plantAt(scene, "plant_boss_0").visible).toBe(true);
  });
  stillMotion(false, () => {
    // 대기업은 이번 스냅샷에서 처음 오는 팀의 화분도 찾는다
    const { scene, props } = layer();
    repot(props, office({}, [team(0, 0.05)]), NOW);
    expect(plantAt(scene).visible).toBe(false);
  });
  stillMotion(true, () => {
    const { scene, props } = layer();
    repot(props, office({}, [team(0, 0.05)]), NOW);
    expect(plantAt(scene).visible).toBe(true);
    expect(tex(scene, "plant_boss_0")).toBe("plant/s0");
  });
});

test("ladder: 시작 시각 전엔 숨어 있다가 LADDER_MS 동안 사규 액자 아래에 서고 걷힌다", () => {
  const { scene, props } = layer();
  const ladder = () => sprites(scene, "ladder/default")[0];
  props.ladder(NOW + 1000, NOW);
  expect(ladder().visible).toBe(false);
  props.tick(NOW + 1000);
  expect(ladder().visible).toBe(true);
  const frame = fp.layouts.large.placements.find((p) => p.id === "rules_frame")!;
  // 액자 바로 아래: 같은 벽, 액자보다 화면 아래
  expect(ladder().y).toBeGreaterThan(frame.y);
  props.tick(NOW + 1000 + LADDER_MS - 1);
  expect(ladder()).toBeDefined();
  props.tick(NOW + 1000 + LADDER_MS);
  expect(ladder()).toBeUndefined();
});
