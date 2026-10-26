// @vitest-environment jsdom
import {
  Container,
  EventBoundary,
  FederatedPointerEvent,
  Graphics,
  Sprite,
  Texture,
  TextureSource,
  type Spritesheet,
} from "pixi.js";
import { expect, test, vi } from "vite-plus/test";
import rawFloorplan from "../../design/sprites/floorplan.json";
import officeJson from "../../design/sprites/office.json";
import owlsJson from "../../design/sprites/owls.json";
import owls3Json from "../../design/sprites/owls@3x.json";
import dogsJson from "../../design/sprites/dogs.json";
import phrases from "../../design/handoff/phrases.ko.json";
import type {
  Bubble,
  EmployeeDto,
  OfficeDto,
  Pose,
  Rank,
  Role,
  Room,
  TeamDto,
} from "../types/ipc.ts";
import {
  ActorLayer,
  bubbleText,
  fnv1a64,
  layoutActors,
  owlFrameKeys,
  roundEven,
  seatProps,
  speciesOf,
  type OwlMeta,
  type Owls,
} from "./actors.ts";
import {
  CHEER_MS,
  CLAP_MS,
  ENTER_MS,
  LADDER_MS,
  STAMP_MS,
  VISIT_STAY_MS,
  WALK_MAX_MS,
  stand,
  walkMs,
  walkPath,
} from "./anim-queue.ts";
import { gridToScreen, parseFloorplan, type Kind } from "./floorplan.ts";
import { toCss } from "./renderer.ts";
import { empKey } from "../ticker.ts";
import { emp as testEmp, team as testTeam, office as testOffice } from "../test-office.ts";

const fp = parseFloorplan(rawFloorplan);
const meta = owlsJson.owlfice as unknown as OwlMeta;
const textures = (keys: string[]) =>
  Object.fromEntries(keys.map((k) => [k, Texture.EMPTY])) as Record<string, Texture>;
const owls: Owls = { textures: textures(Object.keys(owlsJson.frames)), meta };
/** 레이어가 시트 밀도와 상관없이 월드 16×24 를 차지하는 배치 (HANDOFF 6.3) */
const fitsWorld = (s: Sprite, m: OwlMeta = meta) =>
  s.pivot.x === m.anchor[0] &&
  s.pivot.y === m.anchor[1] &&
  s.scale.x === 1 / m.density &&
  s.scale.y === 1 / m.density;
/** head 기준점(텍셀)의 발 기준 월드 좌표 */
const headOffset = (name: string, m: OwlMeta = meta) => [
  (m.anchors[name].head[0] - m.anchor[0]) / m.density,
  (m.anchors[name].head[1] - m.anchor[1]) / m.density,
];
const officeSheet = {
  data: officeJson,
  textures: textures(Object.keys(officeJson.frames)),
} as unknown as Spritesheet;

// ---------- 도우미 ----------

const SINCE = "2026-10-21T10:00:00Z";
const NOW = Date.parse(SINCE) + 75_000;

function emp(
  agentId: string,
  room: Room,
  pose: Pose = "Sit",
  o: Partial<EmployeeDto> & { rank?: Rank; role?: Role } = {},
): EmployeeDto {
  return {
    agentId,
    name: "박서준",
    rank: "Daeri",
    role: "Backend",
    model: null,
    description: null,
    activity: {
      sentence: "일하는 중",
      room,
      since: SINCE,
      expectedSecs: null,
      pose,
      confidence: "Confirmed",
      extra: 0,
      elapsed: false,
    },
    recent: [],
    counters: { read: 0, edit: 0, shell: 0, outside: 0, meeting: 0, errors: 0 },
    uniqueReads: 0,
    pendingCount: 0,
    uniqueEdits: 0,
    planning: false,
    rejected: false,
    medal: false,
    items: [],
    buildTestBorder: "Plain",
    lastSay: null,
    hiredAt: SINCE,
    retiredAt: null,
    departedAt: null,
    seat: null,
    parent: null,
    depth: 1,
    confidence: "Confirmed",
    contextUsed: null,
    contextWindow: null,
    bubble: null,
    jobs: 0,
    meeting: null,
    review: null,
    ...o,
  };
}

function team(teamId: string, employees: EmployeeDto[], o: Partial<TeamDto> = {}): TeamDto {
  return {
    daylight: "day",
    teamId,
    title: "제목",
    building: "A",
    hostBadge: null,
    delayed: false,
    folder: "app",
    started: SINCE,
    lastActivity: SINCE,
    phase: "Working",
    workType: "NewDev",
    deskSlot: 0,
    employees,
    props: {} as TeamDto["props"],
    incident: null,
    idleAlive: false,
    testsToday: { passed: 0, failed: 0 },
    commitsToday: 0,
    ...o,
  };
}

function office(teams: TeamDto[]): OfficeDto {
  return {
    roomRecent: [],
    stateSeq: 1,
    officeId: "s",
    rebuilt: false,
    teams,
    shared: {} as OfficeDto["shared"],
    cues: [],
  };
}

const boss = (room: Room = "TeamLead", pose: Pose = "Sit") =>
  emp("boss", room, pose, { rank: "Bujang" });

function at(kind: Kind, o: OfficeDto, id: string) {
  return layoutActors(fp, kind, o).find((p) => p.emp.agentId === id);
}

function screen(gx: number, gy: number, z = 0) {
  const p = gridToScreen(gx, gy, z);
  return { x: Math.round(p.x), y: Math.round(p.y) };
}

function newLayer(o: { owls?: Owls | null } = {}) {
  const overlay = document.createElement("div");
  const layer = new ActorLayer(fp, officeSheet, o.owls === undefined ? owls : o.owls, overlay);
  const world = new Container();
  layer.attach(world);
  return { layer, overlay, world };
}

// ---------- 종 ----------

test("팀 포커스는 부장을 2초 표시하고 활동 갱신으로 연장되지 않는다", () => {
  const { layer, world } = newLayer();
  const teams = [0, 1].map((deskSlot) =>
    testTeam(
      `t${deskSlot}`,
      [
        testEmp("boss", {
          rank: "Bujang",
          activity: { ...testEmp("boss").activity, room: "TeamLead", pose: "Sit" },
        }),
      ],
      { deskSlot },
    ),
  );
  layer.sync(testOffice("vendor/Claude", 1, teams), "small", NOW);
  layer.highlightTeam("t0", NOW);
  const ring = world.children[world.children.length - 1];
  expect(layer.focusedTeam(NOW + 1999)).toBe("t0");
  expect(ring.visible).toBe(true);
  expect({ x: ring.x, y: ring.y }).toEqual({
    x: layer.rootOf("t0", "boss")!.x,
    y: layer.rootOf("t0", "boss")!.y,
  });
  layer.sync(testOffice("vendor/Claude", 2, [...teams].reverse()), "small", NOW + 1000);
  layer.update(NOW + 2000);
  expect(layer.focusedTeam(NOW + 2000)).toBeNull();
  expect(ring.visible).toBe(false);
  layer.highlightTeam("t1", NOW + 3000);
  expect({ x: ring.x, y: ring.y }).toEqual({
    x: layer.rootOf("t1", "boss")!.x,
    y: layer.rootOf("t1", "boss")!.y,
  });
  layer.sync({ ...testOffice("vendor/Claude", 3, teams), rebuilt: true }, "small", NOW + 3100);
  expect(layer.focusedTeam(NOW + 3100)).toBeNull();
  layer.highlightTeam("t0", NOW + 4000);
  layer.sync(testOffice("vendor/Codex", 1, teams), "small", NOW + 4100);
  expect(layer.focusedTeam(NOW + 4100)).toBeNull();
});

test("종 해시는 백엔드 fnv1a64 와 같은 값이다 (names.rs 테스트 벡터)", () => {
  expect(fnv1a64("")).toBe(0xcbf29ce484222325n);
  expect(fnv1a64("a")).toBe(0xaf63dc4c8601ec8cn);
  // UTF-8 바이트로 센다 (Rust str::bytes)
  expect(fnv1a64("김")).toBe(0x97638a1bac074b24n);
  // 0xaf63dc4c8601ec8c % 5 = 1, 0xcbf29ce484222325 % 5 = 2
  expect(speciesOf("a", meta.employeeSpecies)).toBe("longeared");
  expect(speciesOf("", meta.employeeSpecies)).toBe("little");
});

// ---------- 자리 ----------

test("개발실 좌석 번호 → 좌석 좌표와 depth. 6~9 는 임시 책상", () => {
  const seats = fp.layouts.large.seats;
  const o = office([
    team("t", [boss(), emp("a", "Dev", "Type", { seat: 3 }), emp("b", "Dev", "Sit", { seat: 7 })]),
  ]);
  const a = at("large", o, "a")!;
  expect(a).toMatchObject({
    ...screen(seats.dev[3].gx, seats.dev[3].gy),
    depth: 11.5,
    spot: "dev_3",
  });
  expect(a.look).toEqual({ pose: "type", dir: "f" });
  expect(at("large", o, "b")).toMatchObject({ spot: "temp_1", depth: seats.temp[1].depth });
});

test("부장은 팀장석: 대기업 하나, 중소기업 deskSlot 0~3 팀장석·4~5 간이·null 은 안 그린다", () => {
  const large = at("large", office([team("t", [boss()])]), "boss")!;
  expect(large).toMatchObject({ ...screen(20.9, 2.15), depth: 23, spot: "boss_0" });
  const small = (slot: number | null) =>
    at("small", office([team("t", [boss()], { deskSlot: slot })]), "boss");
  const s = fp.layouts.small.seats;
  expect(small(2)).toMatchObject({ ...screen(s.boss[2].gx, s.boss[2].gy), spot: "boss_2" });
  expect(small(5)).toMatchObject({ ...screen(s.spare[1].gx, s.spare[1].gy), spot: "spare_1" });
  expect(small(null)).toBeUndefined();
});

test("방·자세 → 자리와 sprite 자세 (HANDOFF 5절 표)", () => {
  const stand = (name: string, i = 0) => {
    const [gx, gy, z = 0] = fp.stands[name][i];
    return { ...screen(gx, gy, z), spot: `${name}_${i}` };
  };
  const placement = (id: string) => fp.layouts.large.placements.find((p) => p.id === id)!;
  const one = (e: EmployeeDto, asBoss = false) =>
    asBoss
      ? at("large", office([team("t", [e])]), e.agentId)
      : at("large", office([team("t", [boss(), e])]), e.agentId);
  const cases: [EmployeeDto, boolean, object, object][] = [
    // 파일 수정 / 응답 대기 / 응답 대기 10초+ / 쪼그림 / 졸음: 자기 자리
    [emp("x", "Dev", "Type", { seat: 0 }), false, { spot: "dev_0" }, { pose: "type", dir: "f" }],
    [boss("TeamLead", "Think"), true, { spot: "boss_0" }, { pose: "think", dir: "f" }],
    [boss("TeamLead", "Duck"), true, { spot: "boss_0" }, { pose: "carry", dir: "f", held: "duck" }],
    [boss("TeamLead", "Sit"), true, { spot: "boss_0" }, { pose: "sit", dir: "f" }],
    // 큰 열람·검색, 셸, 테스트, 리뷰, MCP, 커밋, 허가 대기, 서브 종료, 졸음
    [emp("x", "Archive"), false, stand("library"), { pose: "carry", dir: "f", held: "folder" }],
    [emp("x", "Build"), false, stand("build_terminal"), { pose: "idle", dir: "f" }],
    [emp("x", "Build", "Squat"), false, stand("build_terminal"), { pose: "crouch", dir: "f" }],
    [emp("x", "Test"), false, stand("test"), { pose: "idle", dir: "f" }],
    [
      emp("x", "Review"),
      false,
      // 참조 렌더(Python round)처럼 220.5 → 220
      { x: 342, y: 220, spot: "review_sofa_0", depth: placement("review_seat").depth },
      { pose: "sit", dir: "f" },
    ],
    [emp("x", "Meeting"), false, stand("meeting"), { pose: "idle", dir: "f" }],
    [
      emp("x", "Shipping"),
      false,
      stand("shipping"),
      { pose: "carry", dir: "f", held: "box_carry" },
    ],
    [boss("Door"), true, stand("door_wait"), { pose: "carry", dir: "f", held: "clipboard" }],
    [
      emp("x", "Pantry", "Sit", { retiredAt: SINCE }),
      false,
      stand("pantry_console"),
      { pose: "sit", dir: "f" },
    ],
    [
      boss("Pantry", "Sleep"),
      true,
      { ...stand("pantry_sofa"), depth: placement("sofa").depth },
      { pose: "sleep", dir: "f" },
    ],
    // 장애: 부장은 0번에서 panic, 팀원은 1번부터 idle
    [boss("Incident"), true, stand("incident", 0), { pose: "panic", dir: "f" }],
    [emp("x", "Incident"), false, stand("incident", 1), { pose: "idle", dir: "f" }],
  ];
  for (const [e, asBoss, where, look] of cases) {
    const p = one(e, asBoss);
    const label = `${e.activity.room}/${e.activity.pose}`;
    expect(p, label).toMatchObject(where);
    expect(p?.look, label).toEqual(look);
  }
  // 외근은 그리지 않는다
  expect(one(emp("x", "Outside"))).toBeUndefined();
});

test("같은 자리를 여럿이 쓰면 다음 자리, 모자라면 마지막 자리에 겹친다", () => {
  const ids = ["a", "b", "c", "d"];
  const o = office([team("t", [boss(), ...ids.map((id) => emp(id, "Test"))])]);
  expect(ids.map((id) => at("large", o, id)!.spot)).toEqual([
    "test_0",
    "test_1",
    "test_2",
    "test_2",
  ]);
  // 좌석이 겹치면(두 팀 같은 번호) 뒤 직원이 다음 좌석으로
  const seats = office([
    team("t1", [boss(), emp("a", "Dev", "Sit", { seat: 1 })]),
    team(
      "t2",
      [emp("boss2", "TeamLead", "Sit", { rank: "Bujang" }), emp("b", "Dev", "Sit", { seat: 1 })],
      {
        deskSlot: 1,
      },
    ),
  ]);
  expect(at("small", seats, "a")!.spot).toBe("dev_1");
  expect(at("small", seats, "b")!.spot).toBe("dev_2");
  // 좌석 없는 직원은 마지막 임시 책상
  expect(at("large", office([team("t", [boss(), emp("z", "Dev")])]), "z")!.spot).toBe("temp_3");
});

test("퇴근한 팀은 그리지 않는다", () => {
  expect(layoutActors(fp, "large", office([team("t", [boss()], { phase: "Off" })]))).toEqual([]);
});

test("결재 대기(Pace)는 부장이 boss_pace 자리에 idle", () => {
  const p = at("large", office([team("t", [boss("TeamLead", "Pace")])]), "boss")!;
  expect(p.spot).toBe("boss_pace_0");
  expect(p.look).toEqual({ pose: "idle", dir: "f" });
});

test("탕비실: 첫째 콘솔 sit, 둘째 안마의자 sleep, 그다음은 서 있고, 넘치면 그리지 않고 마지막 자리 위에 +n", () => {
  const ids = ["a", "b", "c", "d", "e"];
  const o = office([
    team("t", [boss(), ...ids.map((id) => emp(id, "Pantry", "Sit", { retiredAt: SINCE }))]),
  ]);
  const crowd = new Map<string, number>();
  const got = layoutActors(fp, "large", o, crowd).filter((p) => p.emp.agentId !== "boss");
  expect(got.map((p) => [p.emp.agentId, p.spot, p.look.pose])).toEqual([
    ["a", "pantry_console_0", "sit"],
    ["b", "pantry_massage_0", "sleep"],
    ["c", "pantry_0", "idle"],
    ["d", "pantry_1", "idle"],
  ]);
  expect(crowd).toEqual(new Map([["pantry_1", 1]]));
  // 캔버스: 넘친 직원은 액터가 없고, 마지막 자리 직원 머리에서 (+14, −6) 에 배지 하나
  const { layer, overlay } = newLayer();
  layer.sync(o, "large", NOW);
  expect(layer.rootOf("t", "e")).toBeUndefined();
  const badge = overlay.querySelectorAll<HTMLElement>(".ow-crowd");
  expect(badge).toHaveLength(1);
  expect(badge[0].textContent).toBe("+1");
  const d = got[3];
  const head = meta.anchors.idle_f_0.head;
  expect(badge[0].style.left).toBe(`${d.x + (head[0] - meta.anchor[0]) / meta.density + 14}px`);
  expect(badge[0].style.top).toBe(`${d.y + (head[1] - meta.anchor[1]) / meta.density - 6}px`);
});

test("자리 초과: 서브 12명이면 개발실 6 + 임시 4 만 겹치지 않게 그리고 마지막 임시 책상 위에 +2", () => {
  const { layer, overlay } = newLayer();
  // 백엔드는 10명째 뒤 직원에게 좌석을 안 준다
  const subs = Array.from({ length: 12 }, (_, i) =>
    emp(`s${i}`, "Dev", "Sit", { seat: i < 10 ? i : null }),
  );
  const o = office([team("t", [boss(), ...subs])]);
  layer.sync(o, "large", NOW);
  const drawn = subs.filter((s) => layer.rootOf("t", s.agentId)?.visible);
  expect(drawn).toHaveLength(10);
  const spots = layoutActors(fp, "large", o)
    .filter((p) => p.emp.agentId !== "boss")
    .map((p) => p.spot);
  expect(new Set(spots).size).toBe(10);
  const badge = overlay.querySelectorAll<HTMLElement>(".ow-crowd");
  expect(badge).toHaveLength(1);
  expect(badge[0].textContent).toBe("+2");
  const last = at("large", o, "s9")!;
  expect(last.spot).toBe("temp_3");
  const head = meta.anchors.sit_f_0.head;
  expect(badge[0].style.left).toBe(`${last.x + (head[0] - meta.anchor[0]) / meta.density + 14}px`);
  expect(badge[0].style.top).toBe(`${last.y + (head[1] - meta.anchor[1]) / meta.density - 6}px`);
  // 만석 쪽지는 그대로
  expect(seatProps(fp, "large", o)).toEqual(new Set([...TEMP_DESKS, "note_full"]));
  // 줄면 숫자가 따라 줄고, 자리가 남으면 배지가 없다
  layer.sync(office([team("t", [boss(), ...subs.slice(0, 11)])]), "large", NOW);
  expect(overlay.querySelector(".ow-crowd")!.textContent).toBe("+1");
  layer.sync(office([team("t", [boss(), ...subs.slice(0, 10)])]), "large", NOW);
  expect(overlay.querySelectorAll(".ow-crowd")).toHaveLength(0);
});

test("deskSlot null 팀의 부장은 어디서든 그리지 않는다", () => {
  for (const room of ["TeamLead", "Incident", "Pantry", "Door"] as const)
    expect(
      at("small", office([team("t", [boss(room)], { deskSlot: null })]), "boss"),
      room,
    ).toBeUndefined();
});

const TEMP_DESKS = ["temp_desk_0", "temp_desk_1", "temp_desk_2", "temp_desk_3"];

test("임시·간이 책상은 쓰는 직원이 있을 때만, 좌석 배정이 없으면 만석 쪽지", () => {
  const dev = (id: string, seat: number | null) => emp(id, "Dev", "Sit", { seat });
  const props = (kind: Kind, teams: TeamDto[]) => seatProps(fp, kind, office(teams));
  expect(props("large", [team("t", [boss(), dev("a", 5)])])).toEqual(new Set());
  expect(props("large", [team("t", [boss(), dev("a", 6), dev("b", 8)])])).toEqual(
    new Set([...TEMP_DESKS, "note_temp"]),
  );
  // 좌석 번호가 없으면 마지막 임시 책상에 겹친다
  expect(props("large", [team("t", [boss(), dev("a", 9), dev("b", null)])])).toEqual(
    new Set([...TEMP_DESKS, "note_full"]),
  );
  // 중소기업 deskSlot 4~5 는 간이 책상
  expect(props("small", [team("t", [boss()], { deskSlot: 5 })])).toEqual(new Set(["spare_desk_1"]));
});

test("책상·쪽지는 좌석 배정으로: 임시 자리 직원·간이 책상 부장이 다른 방에 가도 남는다", () => {
  const props = (kind: Kind, teams: TeamDto[]) => seatProps(fp, kind, office(teams));
  const away = emp("a", "Meeting", "Sit", { seat: 7 });
  expect(at("large", office([team("t", [boss(), away])]), "a")!.spot).not.toMatch(/^temp_/);
  expect(props("large", [team("t", [boss(), away])])).toEqual(
    new Set([...TEMP_DESKS, "note_temp"]),
  );
  // 외근이라 그리지 않아도 배정은 그대로
  expect(props("large", [team("t", [boss(), emp("a", "Outside", "Sit", { seat: 7 })])])).toEqual(
    new Set([...TEMP_DESKS, "note_temp"]),
  );
  // 좌석 없는 직원은 다른 방에 있어도 만석
  expect(props("large", [team("t", [boss(), emp("a", "Test", "Sit", { seat: null })])])).toEqual(
    new Set([...TEMP_DESKS, "note_full"]),
  );
  expect(props("small", [team("t", [boss("Meeting")], { deskSlot: 4 })])).toEqual(
    new Set(["spare_desk_0"]),
  );
  // 퇴장한 서브(백엔드가 자리를 비움)와 퇴근한 팀은 책상을 켜지 않는다
  expect(
    props("large", [team("t", [boss(), emp("r", "Pantry", "Sit", { retiredAt: SINCE })])]),
  ).toEqual(new Set());
  expect(
    props("large", [team("t", [boss(), emp("a", "Dev", "Sit", { seat: 7 })], { phase: "Off" })]),
  ).toEqual(new Set());
});

test("은행가 반올림: .5 는 짝수 쪽 (참조 렌더 Python round)", () => {
  expect([0.5, 1.5, 2.5, 220.5, 221.5, -0.5, -1.5].map(roundEven)).toEqual([
    0, 2, 2, 220, 222, 0, -2,
  ]);
  expect(roundEven(2.4)).toBe(2);
  expect(roundEven(2.6)).toBe(3);
});

// ---------- 프레임 ----------

test("담당 겹: 수습은 없고, 담당 프레임 없는 자세는 건너뛴다", () => {
  const type = { pose: "type", dir: "f" } as const;
  expect(owlFrameKeys(owls.textures, "eagle", "daeri-claude", null, type, 1)).toEqual([
    "body/eagle/type_f_1",
    "outfit/daeri-claude/type_f_1",
    null,
  ]);
  // 색상표(frontend)는 type 자세에 담당 프레임이 없다
  expect(owlFrameKeys(owls.textures, "eagle", "daeri-claude", "frontend", type, 0)[2]).toBeNull();
  expect(owlFrameKeys(owls.textures, "eagle", "daeri-claude", "backend", type, 0)[2]).toBe(
    "role/backend/type_f_0",
  );
});

// ---------- 액터 층 ----------

test("같은 agent_id 는 액터를 다시 쓰고, 목록에서 빠지면 지운다", () => {
  const { layer, world } = newLayer();
  const o1 = office([team("t", [boss(), emp("a", "Dev", "Type", { seat: 0 })])]);
  layer.sync(o1, "large", NOW);
  const root = layer.rootOf("t", "a")!;
  expect(world.children).toContain(root);
  // 방이 바뀌어도 같은 컨테이너, 걸어서 옮긴다
  layer.sync(office([team("t", [boss(), emp("a", "Test")])]), "large", NOW);
  expect(layer.rootOf("t", "a")).toBe(root);
  layer.update(NOW + WALK_MAX_MS);
  expect(root.position.x).toBe(screen(...(fp.stands.test[0] as [number, number])).x);
  // 외근은 출구까지 걸어가 숨는다
  layer.sync(office([team("t", [boss(), emp("a", "Outside")])]), "large", NOW + WALK_MAX_MS);
  expect(layer.rootOf("t", "a")).toBe(root);
  layer.update(NOW + 2 * WALK_MAX_MS);
  expect(root.visible).toBe(false);
  layer.sync(office([team("t", [boss()])]), "large", NOW);
  expect(layer.rootOf("t", "a")).toBeUndefined();
  expect(root.destroyed).toBe(true);
});

test("기본·착용 겹을 같은 자리에, 수습은 담당·근속·소품 겹을 숨기고 depth 는 zIndexOf 로 배치 뒤", () => {
  const { layer } = newLayer();
  layer.sync(
    office([team("t", [boss(), emp("a", "Dev", "Type", { seat: 0, role: "Trainee" })])]),
    "large",
    NOW,
  );
  const root = layer.rootOf("t", "a")!;
  const sprites = root.children.filter((c) => c instanceof Sprite);
  expect(sprites).toHaveLength(8);
  expect(sprites.map((s) => s.visible)).toEqual([
    true,
    true,
    false,
    false,
    false,
    false,
    false,
    false,
  ]);
  expect(sprites.every((s) => fitsWorld(s))).toBe(true);
  // 의자(8.7) 뒤, 책상(9.2) 앞
  const n = fp.layouts.large.placements.length;
  expect(root.zIndex).toBe(8900 * 10000 + n + 1);
});

test("도착하면 걸어온 방향과 상관없이 뒤집지 않는다 (같은 스냅샷은 같은 모습)", () => {
  const { layer } = newLayer();
  // 테스트실(x≈350) → 서고(x≈212): 왼쪽으로 걸었어도 도착하면 그대로
  layer.sync(office([team("t", [boss(), emp("a", "Test")])]), "large", NOW);
  const root = layer.rootOf("t", "a")!;
  layer.sync(office([team("t", [boss(), emp("a", "Archive")])]), "large", NOW);
  layer.update(NOW + WALK_MAX_MS);
  expect(root.scale.x).toBe(1);
});

test("이름표: head 기준점을 CSS px 로, 이름 직급 + {elapsed} 를 채운 문장", () => {
  const { layer, overlay } = newLayer();
  const a = emp("a", "Test", "Sit", {
    rank: "Gwajang",
    activity: {
      sentence: "`sleep {elapsed}` 돌리는 중 · {elapsed}째",
      room: "Test",
      since: SINCE,
      expectedSecs: null,
      pose: "Sit",
      confidence: "Confirmed",
      extra: 0,
      elapsed: true,
    },
  });
  layer.sync(office([team("t", [boss(), a])]), "large", NOW);
  layer.placeOverlay((x, y) => toCss(x, y, 2, 1.25));
  layer.hover(empKey("t", "a"));
  layer.update(NOW);
  const tag = overlay.querySelector<HTMLElement>(".ow-nametag")!;
  expect(tag.hidden).toBe(false);
  expect(tag.querySelector("b")!.textContent).toBe("박서준 과장");
  expect(tag.querySelector("span")!.textContent).toBe("`sleep {elapsed}` 돌리는 중 · 1분 15초째");
  // idle_f_0 의 head 기준점 (텍셀) 을 발 기준 월드 좌표로
  const foot = screen(...(fp.stands.test[0] as [number, number]));
  const [hx, hy] = headOffset("idle_f_0");
  const head = toCss(foot.x + hx, foot.y + hy, 2, 1.25);
  expect(tag.style.left).toBe(`${head.left}px`);
  expect(tag.style.top).toBe(`${head.top}px`);
  layer.hover(null);
  expect(tag.hidden).toBe(true);
});

test("{elapsed} 는 Activity.elapsed 일 때만 채운다", () => {
  const { layer, overlay } = newLayer();
  const a = emp("a", "Test");
  a.activity.sentence = "{elapsed} 초 걸렸습니다";
  layer.sync(office([team("t", [boss(), a])]), "large", NOW);
  layer.hover(empKey("t", "a"));
  expect(overlay.querySelector(".ow-nametag span")!.textContent).toBe("{elapsed} 초 걸렸습니다");
});

test("직원을 누르면 onPick 에 (teamId, agentId) 를 준다", () => {
  const { layer } = newLayer();
  const picked: [string, string][] = [];
  layer.onPick = (t, a) => picked.push([t, a]);
  layer.sync(
    office([team("t", [boss(), emp("a", "Test")]), team("u", [emp("a", "Test")], { deskSlot: 1 })]),
    "small",
    NOW,
  );
  layer.rootOf("u", "a")!.emit("pointertap", {} as never);
  layer.rootOf("t", "a")!.emit("pointertap", {} as never);
  expect(picked).toEqual([
    ["u", "a"],
    ["t", "a"],
  ]);
});

test("스냅샷으로 다른 자리에 옮겨지면 이름표를 닫는다", () => {
  const { layer, overlay } = newLayer();
  layer.sync(office([team("t", [boss(), emp("a", "Test")])]), "large", NOW);
  layer.hover(empKey("t", "a"));
  const tag = overlay.querySelector<HTMLElement>(".ow-nametag")!;
  // 같은 자리면 그대로
  layer.sync(office([team("t", [boss(), emp("a", "Test", "Type")])]), "large", NOW);
  expect(tag.hidden).toBe(false);
  layer.sync(office([team("t", [boss(), emp("a", "Archive")])]), "large", NOW);
  expect(tag.hidden).toBe(true);
});

test("짧은 열람은 자리에서 read 자세, 병렬 호출은 +n 배지", () => {
  const { layer, overlay } = newLayer();
  const a = emp("a", "Dev", "Read", { seat: 0 });
  a.activity.extra = 2;
  layer.sync(office([team("t", [boss(), a])]), "large", NOW);
  expect(at("large", office([team("t", [boss(), a])]), "a")).toMatchObject({
    spot: "dev_0",
    look: { pose: "read", dir: "f" },
  });
  const badge = overlay.querySelector<HTMLElement>(".ow-parallel")!;
  expect(badge.textContent).toBe("+2");
  // read_f_0 의 head 기준점에서 배지는 (+14, +6)
  const seat = fp.layouts.large.seats.dev[0];
  const foot = screen(seat.gx, seat.gy);
  const [hx, hy] = headOffset("read_f_0");
  expect(badge.style.left).toBe(`${foot.x + hx + 14}px`);
  expect(badge.style.top).toBe(`${foot.y + hy + 6}px`);
  a.activity.extra = 0;
  layer.sync(office([team("t", [boss(), a])]), "large", NOW);
  expect(overlay.querySelectorAll(".ow-parallel")).toHaveLength(0);
});

test("졸음 자세에는 Zzz, 깨면 지운다", () => {
  const { layer, overlay } = newLayer();
  layer.sync(office([team("t", [boss("Pantry", "Sleep")])]), "large", NOW);
  expect(overlay.querySelectorAll(".ow-zz")).toHaveLength(1);
  layer.sync(office([team("t", [boss("TeamLead", "Type")])]), "large", NOW);
  expect(overlay.querySelectorAll(".ow-zz")).toHaveLength(0);
});

test("아틀라스가 없으면 대체 몸체", () => {
  const { layer } = newLayer({ owls: null });
  layer.sync(office([team("t", [boss()])]), "large", NOW);
  expect(layer.rootOf("t", "boss")!.children[0]).toBeInstanceOf(Graphics);
});

// ---------- 말풍선 ----------

const bub = (say: Bubble["say"], at = NOW - 1000): Bubble => ({
  at: new Date(at).toISOString(),
  say,
});
const bubbleOf = (overlay: HTMLElement) => overlay.querySelector<HTMLElement>(".ow-bubble");

test("종류마다 고른 문구가 phrases 에 있고 재료가 채워진다", () => {
  const b = phrases.bubble as Record<string, unknown>;
  const kinds: [Bubble["say"], string][] = [
    [{ kind: "Rank" }, "rank"],
    [{ kind: "Incident" }, "incident"],
    [{ kind: "AllGreen" }, "allGreen"],
    [{ kind: "TooLong" }, "tooLong"],
    [{ kind: "ThinkingDuck" }, "thinkingDuck"],
    [{ kind: "PlanApproval" }, "planApproval"],
    [{ kind: "Scratch" }, "scratch"],
    [{ kind: "Compaction" }, "compaction"],
    [{ kind: "UnknownTool" }, "unknownTool"],
    [{ kind: "ScriptFallback" }, "scriptFallback"],
    [{ kind: "UnknownResult" }, "unknownResult"],
  ];
  for (const [say, key] of kinds) {
    if (key === "rank" || key === "unknownTool") continue;
    expect([b[key]].flat(), say.kind).toContain(bubbleText("a", "Daeri", bub(say)));
  }
  // 직급별 풀 (부장·사원)
  for (const key of ["rank", "unknownTool"]) {
    const byRank = b[key] as Record<string, string[]>;
    const say = { kind: key === "rank" ? "Rank" : "UnknownTool" } as Bubble["say"];
    expect(byRank.bujang, key).toContain(bubbleText("a", "Bujang", bub(say)));
    expect(byRank.sawon, key).toContain(bubbleText("a", "Sawon", bub(say)));
  }
  // 직급별 풀은 그 직급 것만
  const sawon = (b.rank as Record<string, string[]>).sawon;
  expect(sawon).toContain(bubbleText("a", "Sawon", bub({ kind: "Rank" })));
  expect(bubbleText("a", "Daeri", bub({ kind: "Mcp", server: "figma", tool: "get_file" }))).toBe(
    "figma와 get_file 건으로",
  );
  const vendor = bubbleText("a", "Daeri", bub({ kind: "NewVendor", server: "figma" }))!;
  expect((b.newVendor as string[]).map((s) => s.replace("{server}", "figma"))).toContain(vendor);
  const skill = bubbleText("a", "Daeri", bub({ kind: "NewSkill", skill: "pdf" }))!;
  expect((b.newSkill as string[]).map((s) => s.replace("{skill}", "pdf"))).toContain(skill);
  expect(bubbleText("a", "Daeri", bub({ kind: "Say", text: "안녕 {x}" }))).toBe("안녕 {x}");
});

test("같은 직원·같은 at 이면 다시 그려도 같은 문구", () => {
  const b = bub({ kind: "Rank" });
  const first = bubbleText("a", "Daeri", b);
  const { layer, overlay } = newLayer();
  const a = emp("a", "Dev", "Type", { seat: 0, bubble: b });
  layer.sync(office([team("t", [boss(), a])]), "large", NOW);
  layer.sync(office([team("t", [boss(), { ...a }])]), "large", NOW);
  expect(overlay.querySelectorAll(".ow-bubble")).toHaveLength(1);
  expect(bubbleOf(overlay)!.textContent).toBe(first);
});

test("4초가 지나면 숨기고, 오래된 스냅샷은 만들지도 않는다", () => {
  const { layer, overlay } = newLayer();
  const a = emp("a", "Dev", "Type", { seat: 0, bubble: bub({ kind: "Say", text: "안녕" }, NOW) });
  layer.sync(office([team("t", [boss(), a])]), "large", NOW);
  expect(bubbleOf(overlay)!.hidden).toBe(false);
  layer.update(NOW + 3999);
  expect(bubbleOf(overlay)!.hidden).toBe(false);
  layer.update(NOW + 4000);
  expect(bubbleOf(overlay)!.hidden).toBe(true);

  const old = emp("b", "Dev", "Type", {
    seat: 1,
    bubble: bub({ kind: "Say", text: "옛날" }, NOW - 4001),
  });
  const fresh = newLayer();
  fresh.layer.sync(office([team("t", [boss(), old])]), "large", NOW);
  expect(bubbleOf(fresh.overlay)).toBeNull();
});

test("모양 클래스: 추정 점선, 장애 상황실, 허가 대기", () => {
  const say = bub({ kind: "Say", text: "x" });
  const shape = (room: Room, confidence: "Confirmed" | "Inferred") => {
    const { layer, overlay } = newLayer();
    const a = emp("a", room, "Sit", { bubble: say });
    a.activity.confidence = confidence;
    layer.sync(office([team("t", [boss(), a])]), "large", NOW);
    return bubbleOf(overlay)!;
  };
  expect(shape("Dev", "Confirmed").className).toBe("ow-bubble");
  expect(shape("Dev", "Inferred").classList.contains("is-estimate")).toBe(true);
  expect(shape("Incident", "Confirmed").classList.contains("is-incident")).toBe(false);
  expect(shape("Door", "Confirmed").classList.contains("is-ask")).toBe(true);
  expect(shape("Dev", "Confirmed").getAttribute("aria-hidden")).toBe("true");
});

test("is-incident 는 방이 아니라 Incident 말풍선에 붙는다", () => {
  const { layer, overlay } = newLayer();
  const a = emp("a", "Dev", "Type", { seat: 0, bubble: bub({ kind: "Incident" }) });
  layer.sync(office([team("t", [boss(), a])]), "large", NOW);
  expect(bubbleOf(overlay)!.classList.contains("is-incident")).toBe(true);
});

test("미래 at 은 4초 넘게 앞서면 안 보인다", () => {
  const near = newLayer();
  const a = emp("a", "Dev", "Type", {
    seat: 0,
    bubble: bub({ kind: "Say", text: "x" }, NOW + 3000),
  });
  near.layer.sync(office([team("t", [boss(), a])]), "large", NOW);
  expect(bubbleOf(near.overlay)!.hidden).toBe(false);
  const far = newLayer();
  const b = emp("b", "Dev", "Type", {
    seat: 0,
    bubble: bub({ kind: "Say", text: "x" }, NOW + 5000),
  });
  far.layer.sync(office([team("t", [boss(), b])]), "large", NOW);
  expect(bubbleOf(far.overlay)).toBeNull();
});

test("직원이 없어지거나 외근이면 말풍선도 지운다", () => {
  const { layer, overlay } = newLayer();
  const a = emp("a", "Dev", "Type", { seat: 0, bubble: bub({ kind: "Say", text: "x" }) });
  layer.sync(office([team("t", [boss(), a])]), "large", NOW);
  expect(bubbleOf(overlay)).not.toBeNull();
  const out = { ...a, activity: { ...a.activity, room: "Outside" as Room } };
  layer.sync(office([team("t", [boss(), out])]), "large", NOW);
  expect(bubbleOf(overlay)).toBeNull();
  layer.sync(office([team("t", [boss(), a])]), "large", NOW);
  layer.sync(office([team("t", [boss()])]), "large", NOW);
  expect(bubbleOf(overlay)).toBeNull();
});

test("사용자 글은 textContent 로만 넣는다", () => {
  const { layer, overlay } = newLayer();
  const a = emp("a", "Dev", "Type", {
    seat: 0,
    bubble: bub({ kind: "Say", text: "<img src=x onerror=alert(1)>" }),
  });
  layer.sync(office([team("t", [boss(), a])]), "large", NOW);
  expect(bubbleOf(overlay)!.querySelector("img")).toBeNull();
  expect(bubbleOf(overlay)!.textContent).toBe("<img src=x onerror=alert(1)>");
});

test("풀에 없는 키는 그리지 않고 경고는 한 번", () => {
  const spy = vi.spyOn(console, "warn").mockImplementation(() => {});
  const bad = { kind: "Nope" } as unknown as Bubble["say"];
  expect(bubbleText("a", "Daeri", bub(bad))).toBeNull();
  expect(bubbleText("a", "Daeri", bub(bad))).toBeNull();
  expect(spy).toHaveBeenCalledTimes(1);
  spy.mockRestore();
});

// ---------- 연출 큐 연결 ----------

const cue = (c: Record<string, unknown>) => ({ teamId: "t", ...c }) as OfficeDto["cues"][number];
const withCues = (o: OfficeDto, cues: OfficeDto["cues"], rebuilt = false): OfficeDto => ({
  ...o,
  cues,
  rebuilt,
});
const posOf = (layer: ActorLayer, id: string, teamId = "t") => {
  const r = layer.rootOf(teamId, id)!;
  return { x: r.position.x, y: r.position.y };
};
const DOOR = fp.stands.door[0] as [number, number];
const SEAT0 = screen(2.75, 6.25);

test("Enter cue 면 문에서 좌석까지 4초 걷고, cue 없이 처음 보는 직원은 제자리", () => {
  const { layer } = newLayer();
  layer.sync(office([team("t", [boss()])]), "large", NOW);
  const o = office([team("t", [boss(), emp("a", "Dev", "Type", { seat: 0 }), emp("b", "Test")])]);
  layer.sync(withCues(o, [cue({ kind: "Enter", agent: "a" })]), "large", NOW);
  expect(posOf(layer, "a")).toEqual(screen(...DOOR));
  expect(posOf(layer, "b")).toEqual(screen(...(fp.stands.test[0] as [number, number])));
  layer.update(NOW + ENTER_MS - 200);
  expect(posOf(layer, "a")).not.toEqual(SEAT0);
  layer.update(NOW + ENTER_MS);
  expect(posOf(layer, "a")).toEqual(SEAT0);
});

// 프레임마다 다른 텍스처라 어떤 자세를 그렸는지 이름으로 본다
const labeled: Owls = {
  textures: Object.fromEntries(
    Object.keys(owlsJson.frames).map((k) => [k, new Texture({ label: k })]),
  ) as Record<string, Texture>,
  meta,
};
const bodyOf = (layer: ActorLayer, id: string, teamId = "t") =>
  (layer.rootOf(teamId, id)!.children[0] as Sprite).texture.label;

test("같은 스냅샷으로 다시 불려도(목록만 바뀜) cue 를 또 재생하지 않는다", () => {
  const { layer } = newLayer({ owls: labeled });
  const sp = speciesOf("boss", meta.employeeSpecies);
  layer.sync(office([team("t", [boss()])]), "large", NOW);
  const o = withCues(office([team("t", [boss()])]), [cue({ kind: "Clap", agent: "boss" })]);
  layer.sync(o, "large", NOW);
  expect(bodyOf(layer, "boss")).toBe(`body/${sp}/clap_f_0`);
  layer.update(NOW + CLAP_MS);
  expect(bodyOf(layer, "boss")).toBe(`body/${sp}/sit_f_0`);
  layer.sync(o, "large", NOW + CLAP_MS);
  expect(bodyOf(layer, "boss")).toBe(`body/${sp}/sit_f_0`);
});

test("rebuilt·instant 는 큐를 비우고 바로 제자리, rebuilt 의 출근 cue 는 재생한다", () => {
  const { layer } = newLayer();
  layer.sync(office([team("t", [boss(), emp("a", "Test")])]), "large", NOW);
  // 걷는 중에 재구성이 오면 바로 서고
  layer.sync(office([team("t", [boss(), emp("a", "Archive")])]), "large", NOW);
  layer.sync(
    withCues(office([team("t", [boss(), emp("a", "Archive")])]), [], true),
    "large",
    NOW + 10,
  );
  expect(posOf(layer, "a")).toEqual(screen(...(fp.stands.library[0] as [number, number])));
  // 사무실 전환 첫 스냅샷: 자리가 바뀌어도, cue 가 있어도 걷지 않는다
  const o = office([team("t", [boss(), emp("a", "Dev", "Type", { seat: 0 })])]);
  layer.sync(withCues(o, [cue({ kind: "Rehire", agent: "a" })]), "large", NOW + 20, {
    instant: true,
  });
  expect(posOf(layer, "a")).toEqual(SEAT0);
  // 방금 시작된 세션의 조용한 재구성: 출근만 연출한다 (docs/CONTRACTS.md C4)
  const o2 = office([
    team("t", [boss(), emp("a", "Dev", "Type", { seat: 0 }), emp("c", "Dev", "Type", { seat: 1 })]),
  ]);
  layer.sync(withCues(o2, [cue({ kind: "Enter", agent: "c" })], true), "large", NOW + 30);
  expect(posOf(layer, "c")).toEqual(screen(...DOOR));
  expect(posOf(layer, "a")).toEqual(SEAT0);
});

test("Retire: 스냅샷이 숨기면 출구로 걸어가 사라지고, 탕비실이면 탕비실로 걷는다", () => {
  const { layer } = newLayer();
  const a = emp("a", "Dev", "Type", { seat: 0 });
  const b = emp("b", "Dev", "Type", { seat: 1 });
  layer.sync(office([team("t", [boss(), a, b])]), "large", NOW);
  const gone = { ...a, activity: { ...a.activity, room: "Outside" as Room } };
  const rest = { ...b, activity: { ...b.activity, room: "Pantry" as Room, pose: "Sit" as Pose } };
  layer.sync(
    withCues(office([team("t", [boss(), gone, rest])]), [
      cue({ kind: "Retire", agent: "a" }),
      cue({ kind: "Retire", agent: "b" }),
    ]),
    "large",
    NOW,
  );
  expect(layer.rootOf("t", "a")!.visible).toBe(true);
  layer.update(NOW + WALK_MAX_MS);
  expect(layer.rootOf("t", "a")!.visible).toBe(false);
  expect(layer.rootOf("t", "b")!.visible).toBe(true);
  expect(posOf(layer, "b")).toEqual(screen(...(fp.stands.pantry_console[0] as [number, number])));
});

test("걷는 동안 walk 8fps, 왼쪽이면 scale.x −1", () => {
  const { layer } = newLayer({ owls: labeled });
  layer.sync(office([team("t", [boss(), emp("a", "Test")])]), "large", NOW);
  // 테스트실 → 서고: 통로에서 서고 entry 로 가는 구간은 왼쪽 위
  layer.sync(office([team("t", [boss(), emp("a", "Archive")])]), "large", NOW);
  layer.update(NOW + 500);
  const sp = speciesOf("a", meta.employeeSpecies);
  expect(bodyOf(layer, "a")).toMatch(new RegExp(`^body/${sp}/walk_b_[0-3]$`));
  expect(layer.rootOf("t", "a")!.scale.x).toBe(-1);
});

test("Rehire: 탕비실에 보이던 서브는 있던 자리에서 걷고, 숨어 있던 직원만 문에서 들어온다", () => {
  const { layer } = newLayer();
  const b = emp("b", "Pantry", "Sit");
  const c = emp("c", "Dev", "Type", { seat: 1 });
  layer.sync(office([team("t", [boss(), b, c])]), "large", NOW);
  const gone = { ...c, activity: { ...c.activity, room: "Outside" as Room } };
  layer.sync(office([team("t", [boss(), b, gone])]), "large", NOW);
  layer.update(NOW + WALK_MAX_MS);
  expect(layer.rootOf("t", "c")!.visible).toBe(false);
  const back = {
    ...b,
    activity: { ...b.activity, room: "Dev" as Room, pose: "Type" as Pose },
    seat: 0,
  };
  const o = office([team("t", [boss(), back, c])]);
  layer.sync(
    withCues(o, [cue({ kind: "Rehire", agent: "b" }), cue({ kind: "Rehire", agent: "c" })]),
    "large",
    NOW + WALK_MAX_MS,
  );
  expect(posOf(layer, "b")).toEqual(screen(...(fp.stands.pantry_console[0] as [number, number])));
  expect(posOf(layer, "c")).toEqual(screen(...DOOR));
});

test("Depart: 퇴근한 서브는 출구까지 걸어가 숨고 좌석 소품에서 빠진다, 재출근하면 문에서 들어온다", () => {
  const { layer } = newLayer();
  const a = emp("a", "Dev", "Type", { seat: 0 });
  layer.sync(office([team("t", [boss(), a])]), "large", NOW);
  // 종료: 탕비실 콘솔에서 쉰다
  const done = {
    ...a,
    retiredAt: SINCE,
    seat: null,
    activity: { ...a.activity, room: "Pantry" as Room, pose: "Sit" as Pose },
  };
  layer.sync(
    withCues(office([team("t", [boss(), done])]), [cue({ kind: "Retire", agent: "a" })]),
    "large",
    NOW,
  );
  layer.update(NOW + WALK_MAX_MS);
  const console0 = stand(fp, "pantry_console");
  expect(posOf(layer, "a")).toEqual(screen(console0.gx, console0.gy, console0.z));
  // 퇴근: 백엔드는 문 앞으로 옮기고 departed_at 을 찍는다. 좌석 배정이 남아 있어도 책상을 켜지 않는다
  const gone = {
    ...done,
    departedAt: SINCE,
    seat: 7,
    activity: { ...done.activity, room: "Door" as Room },
  };
  const o = withCues(office([team("t", [boss(), gone])]), [cue({ kind: "Depart", agent: "a" })]);
  expect(at("large", o, "a")).toBeUndefined();
  expect(seatProps(fp, "large", o)).toEqual(new Set());
  const t1 = NOW + WALK_MAX_MS;
  layer.sync(o, "large", t1);
  const root = layer.rootOf("t", "a")!;
  const out = walkMs(walkPath(fp, console0, stand(fp, "exit")));
  layer.update(t1 + out - 1);
  expect(root.visible).toBe(true);
  layer.update(t1 + out);
  expect(root.visible).toBe(false);
  // 숨어도 액터는 남아 재출근 때 문에서 들어온다
  const back = { ...a, seat: 0 };
  const t2 = t1 + out;
  layer.sync(
    withCues(office([team("t", [boss(), back])]), [cue({ kind: "Rehire", agent: "a" })]),
    "large",
    t2,
  );
  expect(layer.rootOf("t", "a")).toBe(root);
  expect(posOf(layer, "a")).toEqual(screen(...DOOR));
  layer.update(t2 + ENTER_MS);
  expect(posOf(layer, "a")).toEqual(SEAT0);
});

test("두 팀에 같은 agent_id 가 있으면 액터가 둘이고 cue 는 team_id 로 그 팀 직원에게만 간다", () => {
  const { layer } = newLayer({ owls: labeled });
  const teams = () => [
    team("t", [boss(), emp("a", "Dev", "Type", { seat: 0 })]),
    team("u", [boss(), emp("a", "Dev", "Type", { seat: 1 })], { deskSlot: 1 }),
  ];
  layer.sync(office(teams()), "large", NOW);
  const ta = layer.rootOf("t", "a")!;
  const ua = layer.rootOf("u", "a")!;
  expect(ta).toBeDefined();
  expect(ua).toBeDefined();
  expect(ta).not.toBe(ua);
  expect(ta.position.x).not.toBe(ua.position.x);
  const uaPos = posOf(layer, "a", "u");
  // u 팀 a 에게만 손뼉: t 팀 a 는 앉은 자세 그대로
  layer.sync(
    withCues(office(teams()), [cue({ teamId: "u", kind: "Clap", agent: "a" })]),
    "large",
    NOW,
  );
  expect(bodyOf(layer, "a", "u")).toMatch(/\/clap_f_0$/);
  expect(bodyOf(layer, "a", "t")).not.toMatch(/\/clap_/);
  expect(posOf(layer, "a", "u")).toEqual(uaPos);
  // 한 팀에서 빠지면 그 팀 액터만 지운다
  layer.sync(office([teams()[1]]), "large", NOW);
  expect(layer.rootOf("t", "a")).toBeUndefined();
  expect(layer.rootOf("u", "a")).toBe(ua);
});

test("두 sync 사이에 지나간 DTO 의 cue(earlier)도 재생한다", () => {
  const { layer } = newLayer();
  layer.sync(office([team("t", [boss()])]), "large", NOW);
  const o = office([team("t", [boss(), emp("a", "Dev", "Type", { seat: 0 })])]);
  layer.sync(o, "large", NOW, { earlier: [cue({ kind: "Enter", agent: "a" })] });
  expect(posOf(layer, "a")).toEqual(screen(...DOOR));
});

// ---------- 사장님 방문·반려 ----------

const visitCue = (text = "배포 상황 보고하세요") => cue({ kind: "BossVisit", text });
const BOSS_IN = [team("t", [boss()])];
const frontOf = (seat: { gx: number; gy: number }) => ({ gx: seat.gx, gy: seat.gy + 2, z: 0 });
// 대기업 부장 좌석 앞까지 문에서 걷는 시간과 나가는 시간
const SEAT_BOSS = fp.layouts.large.seats.boss[0];
const ARRIVE = walkMs(walkPath(fp, stand(fp, "door"), frontOf(SEAT_BOSS)));
const LEAVE = walkMs(walkPath(fp, frontOf(SEAT_BOSS), stand(fp, "exit")));

test("BossVisit cue 로 사장님 액터가 생기고 도착 5초 뒤 나가 사라진다, 부장은 펄쩍, 인터폰이 울린다", () => {
  const { layer, overlay, world } = newLayer({ owls: labeled });
  const ring = vi.fn();
  layer.onRing = ring;
  const sp = speciesOf("boss", meta.employeeSpecies);
  layer.sync(office(BOSS_IN), "large", NOW);
  const base = world.children.length;
  layer.sync(withCues(office(BOSS_IN), [visitCue()]), "large", NOW);
  expect(world.children).toHaveLength(base + 1);
  expect(ring).toHaveBeenCalledWith(NOW, expect.objectContaining({ kind: "BossVisit" }));
  expect(bodyOf(layer, "boss")).toBe(`body/${sp}/jump_f_0`);
  const visitor = world.children[base] as Container;
  expect((visitor.children[0] as Sprite).texture.label).toMatch(
    new RegExp(`^body/${meta.bossSpecies}/walk_[fb]_`),
  );
  expect((visitor.children[1] as Sprite).texture.label).toMatch(/^outfit\/boss\/walk_[fb]_/);
  // 걷는 동안은 말풍선이 없고, 도착하면 보인다
  const bubble = overlay.querySelector<HTMLElement>(".ow-bubble")!;
  expect(bubble.textContent).toBe("배포 상황 보고하세요");
  layer.update(NOW + 100);
  expect(bubble.hidden).toBe(true);
  layer.update(NOW + ARRIVE - 1);
  expect(bubble.hidden).toBe(true);
  layer.update(NOW + ARRIVE);
  expect(bubble.hidden).toBe(false);
  expect(world.children).toHaveLength(base + 1);
  // 도착 5초 뒤 문으로 나가 사라진다
  layer.update(NOW + ARRIVE + VISIT_STAY_MS - 1);
  expect(world.children).toHaveLength(base + 1);
  layer.update(NOW + ARRIVE + VISIT_STAY_MS);
  expect(world.children).toHaveLength(base + 1);
  layer.update(NOW + ARRIVE + VISIT_STAY_MS + LEAVE);
  expect(world.children).toHaveLength(base);
  expect(overlay.querySelector(".ow-bubble")).toBeNull();
});

test("BossVisit: instant(사무실 전환·숨김 복귀)면 사장님이 오지 않고, 말풍선은 30자에서 자른다", () => {
  const { layer, overlay, world } = newLayer();
  const ring = vi.fn();
  layer.onRing = ring;
  layer.sync(office(BOSS_IN), "large", NOW);
  const base = world.children.length;
  layer.sync(withCues(office(BOSS_IN), [visitCue()]), "large", NOW + 1, { instant: true });
  expect(world.children).toHaveLength(base);
  expect(ring).not.toHaveBeenCalled();
  // 조용한 재구성에 남은 방문 cue 도 지나간 일이다
  layer.sync(withCues(office(BOSS_IN), [visitCue()], true), "large", NOW + 2);
  expect(world.children).toHaveLength(base);
  layer.sync(withCues(office(BOSS_IN), [visitCue("가".repeat(40))]), "large", NOW + 3);
  expect(overlay.querySelector(".ow-bubble")!.textContent).toBe(`${"가".repeat(30)}…`);
});

test("Reject cue: 부장이 머리를 긁고, 결재판 자리에 도장이 1초 보인다", () => {
  const { layer } = newLayer({ owls: labeled });
  layer.sync(office(BOSS_IN), "large", NOW);
  layer.sync(withCues(office(BOSS_IN), [cue({ kind: "Reject", agent: "boss" })]), "large", NOW);
  const stamp = layer.rootOf("t", "boss")!.children.slice(-1)[0] as Sprite;
  expect(bodyOf(layer, "boss")).toMatch(/\/walk_b_/);
  expect(stamp.visible).toBe(false);
  const seat = fp.layouts.large.seats.boss[0];
  const arrive =
    NOW + walkMs(walkPath(fp, { gx: seat.gx, gy: seat.gy, z: 0 }, stand(fp, "security")));
  layer.update(arrive);
  expect(bodyOf(layer, "boss")).toMatch(/\/scratch_f_/);
  expect(stamp.visible).toBe(true);
  layer.update(arrive + STAMP_MS - 1);
  expect(stamp.visible).toBe(true);
  layer.update(arrive + STAMP_MS);
  expect(stamp.visible).toBe(false);
  // 조용한 재구성에서는 다시 보이지 않는다
  layer.sync(
    withCues(office(BOSS_IN), [cue({ kind: "Reject", agent: "boss" })], true),
    "large",
    NOW + 5000,
  );
  expect(stamp.visible).toBe(false);
  expect(bodyOf(layer, "boss")).toMatch(/\/sit_f_/);
});

test("움직임 줄이기: 사장님은 걷지 않고 바로 서고 도착 정확히 5초 뒤 사라진다", () => {
  vi.stubGlobal("matchMedia", () => ({ matches: true }));
  try {
    const { layer, world } = newLayer();
    layer.sync(office(BOSS_IN), "large", NOW);
    const base = world.children.length;
    layer.sync(withCues(office(BOSS_IN), [visitCue()]), "large", NOW);
    expect(world.children).toHaveLength(base + 1);
    const v = world.children[base] as Container;
    const to = gridToScreen(SEAT_BOSS.gx, SEAT_BOSS.gy + 2);
    expect({ x: v.position.x, y: v.position.y }).toEqual({
      x: roundEven(to.x),
      y: roundEven(to.y),
    });
    layer.update(NOW + VISIT_STAY_MS - 1);
    expect(world.children).toHaveLength(base + 1);
    layer.update(NOW + VISIT_STAY_MS);
    expect(world.children).toHaveLength(base);
  } finally {
    vi.unstubAllGlobals();
  }
});

test("사장님은 방문 중 사무실을 옮기거나 그 팀이 퇴근하면 바로 없다, 퇴근한 팀에는 오지 않는다", () => {
  const { layer, overlay, world } = newLayer();
  const ring = vi.fn();
  layer.onRing = ring;
  layer.sync(office(BOSS_IN), "large", NOW);
  const base = world.children.length;
  const visit = () => withCues(office(BOSS_IN), [visitCue()]);
  layer.sync(visit(), "large", NOW);
  expect(world.children).toHaveLength(base + 1);
  layer.sync(office(BOSS_IN), "large", NOW + 100, { instant: true });
  expect(world.children).toHaveLength(base);
  expect(overlay.querySelector(".ow-bubble")).toBeNull();
  layer.sync(visit(), "large", NOW + 200);
  expect(world.children).toHaveLength(base + 1);
  layer.sync(office([team("t", [boss()], { phase: "Off" })]), "large", NOW + 300);
  expect(world.children).toHaveLength(base);
  // 퇴근한 팀은 인터폰도 부장 펄쩍도 없다
  ring.mockClear();
  layer.sync(
    withCues(office([team("t", [boss()], { phase: "Off" })]), [visitCue()]),
    "large",
    NOW + 400,
  );
  expect(ring).not.toHaveBeenCalled();
  expect(world.children).toHaveLength(base);
});

test("사장님 자리는 그 팀 부장의 실제 좌석 앞: 팀장석 3번·간이 책상", () => {
  const stands = (slot: number) => {
    const { layer, world } = newLayer();
    const t = [team("t", [boss()], { deskSlot: slot })];
    layer.sync(office(t), "small", NOW);
    layer.sync(withCues(office(t), [visitCue()]), "small", NOW);
    layer.update(NOW + 60_000 - VISIT_STAY_MS);
    const v = world.children[world.children.length - 1] as Container;
    return { x: v.position.x, y: v.position.y };
  };
  const s = fp.layouts.small.seats;
  for (const [slot, seat] of [
    [3, s.boss[3]],
    [4, s.spare[0]],
    [5, s.spare[1]],
  ] as const) {
    const want = gridToScreen(seat.gx, seat.gy + 2);
    expect(stands(slot)).toEqual({ x: roundEven(want.x), y: roundEven(want.y) });
  }
});

test("같은 팀에 BossVisit 이 또 오면 앞 사장님을 없애고 하나만 둔다", () => {
  const { layer, overlay, world } = newLayer();
  layer.sync(office(BOSS_IN), "large", NOW);
  const base = world.children.length;
  layer.sync(withCues(office(BOSS_IN), [visitCue("하나")]), "large", NOW, {
    earlier: [cue({ kind: "BossVisit", text: "앞" })],
  });
  expect(world.children).toHaveLength(base + 1);
  expect(overlay.querySelectorAll(".ow-bubble")).toHaveLength(1);
  expect(overlay.querySelector(".ow-bubble")!.textContent).toBe("하나");
  layer.sync(withCues(office(BOSS_IN), [visitCue("둘")]), "large", NOW + 1000);
  expect(world.children).toHaveLength(base + 1);
  expect(overlay.querySelector(".ow-bubble")!.textContent).toBe("둘");
});

test("장면을 다시 지어 attach 하면 방문 중인 사장님 말풍선이 오버레이에 다시 붙는다", () => {
  const { layer, overlay } = newLayer();
  layer.sync(office(BOSS_IN), "large", NOW);
  layer.sync(withCues(office(BOSS_IN), [visitCue()]), "large", NOW);
  overlay.querySelector(".ow-bubble")!.remove();
  layer.attach(new Container());
  expect(overlay.querySelector(".ow-bubble")).not.toBeNull();
});

test("사장님 클릭은 방문한 팀을 전달하고 직원 클릭으로 전달하지 않는다", () => {
  const { layer, world } = newLayer();
  const pick = vi.fn();
  const bossPick = vi.fn();
  layer.onPick = pick;
  layer.onBossPick = bossPick;
  layer.sync(office(BOSS_IN), "large", NOW);
  const base = world.children.length;
  layer.sync(withCues(office(BOSS_IN), [visitCue()]), "large", NOW);
  const visitor = world.children[base];
  visitor.emit("pointertap", new FederatedPointerEvent(new EventBoundary()));
  expect(bossPick).toHaveBeenCalledWith("t", visitor.x, visitor.y);
  expect(pick).not.toHaveBeenCalled();
});

test("반려 도장은 걷는 동안 숨고 긁기가 시작된 때부터 1초 보인다", () => {
  const { layer } = newLayer({ owls: labeled });
  layer.sync(office(BOSS_IN), "large", NOW);
  const moved = [team("t", [boss("TeamLead", "Pace")])];
  layer.sync(withCues(office(moved), [cue({ kind: "Reject", agent: "boss" })]), "large", NOW);
  const root = layer.rootOf("t", "boss")!;
  const stamp = root.children.slice(-1)[0] as Sprite;
  expect(bodyOf(layer, "boss")).toMatch(/\/walk_/);
  expect(stamp.visible).toBe(false);
  // 걷기(0.5초 이상)가 끝나면 긁는다
  layer.update(NOW + WALK_MAX_MS);
  expect(bodyOf(layer, "boss")).toMatch(/\/scratch_/);
  expect(stamp.visible).toBe(true);
  layer.update(NOW + WALK_MAX_MS + STAMP_MS);
  expect(stamp.visible).toBe(false);
});

// ---------- 빌드실·테스트실·출고장 (T25) ----------

const stamp = (o: { passed?: number; failed?: number; estimated?: boolean } = {}) =>
  cue({
    kind: "Stamp",
    confirmedPass: true,
    agent: "a",
    at: SINCE,
    passed: 12,
    failed: 0,
    estimated: false,
    ...o,
  });
// 테스트실에서 일하던 직원이 도장 때 개발실로 돌아간다
const testerLeaves = (cues: OfficeDto["cues"]) => {
  const { layer } = newLayer({ owls: labeled });
  const sp = speciesOf("a", meta.employeeSpecies);
  layer.sync(office([team("t", [boss(), emp("a", "Test")])]), "large", NOW);
  const back = office([team("t", [boss(), emp("a", "Dev", "Type", { seat: 0 })])]);
  layer.sync(withCues(back, cues), "large", NOW);
  return { layer, pose: () => bodyOf(layer, "a")!.replace(`body/${sp}/`, "") };
};

test("합격 도장: 테스트실에 있던 직원이 만세 1초 뒤 자리로 걷는다", () => {
  const { layer, pose } = testerLeaves([stamp()]);
  expect(pose()).toBe("cheer_f_0");
  expect(posOf(layer, "a")).toEqual(screen(...(fp.stands.test[0] as [number, number])));
  layer.update(NOW + CHEER_MS);
  expect(pose()).toMatch(/^walk_/);
});

test("실패·추정 도장과 도장 없는 이동에는 만세가 없다", () => {
  for (const cues of [[stamp({ failed: 2 })], [stamp({ estimated: true })], []])
    expect(testerLeaves(cues).pose()).toMatch(/^walk_/);
});

test("타건 여부는 현재 보이는 자세만 보고 걷는 중·퇴근한 직원은 제외한다", () => {
  const { layer } = newLayer();
  layer.sync(office([team("t", [boss(), emp("a", "Test", "Type")])]), "large", NOW);
  expect(layer.isTyping()).toBe(false);
  layer.sync(office([team("t", [boss(), emp("a", "Dev", "Type", { seat: 0 })])]), "large", NOW + 1);
  expect(layer.isTyping()).toBe(false);
  layer.update(NOW + 1 + WALK_MAX_MS);
  expect(layer.isTyping()).toBe(true);
  layer.sync(office([]), "large", NOW + 2 + WALK_MAX_MS, { instant: true });
  expect(layer.isTyping()).toBe(false);
});

test("새 cue 는 onCue 로 넘기되 사무실 전환·숨은 창 복귀(instant)면 넘기지 않는다", () => {
  const { layer } = newLayer();
  const seen: string[] = [];
  layer.onCue = (c) => void seen.push(c.kind);
  const o = withCues(office([team("t", [boss()])]), [stamp(), cue({ kind: "Truck" })]);
  layer.sync(o, "large", NOW);
  expect(seen).toEqual(["Stamp", "Truck"]);
  // 같은 스냅샷으로 다시 불려도 또 넘기지 않는다
  layer.sync(o, "large", NOW + 10);
  expect(seen).toHaveLength(2);
  layer.sync(
    withCues(office([team("t", [boss()])]), [cue({ kind: "Fireworks" })]),
    "large",
    NOW + 20,
    { instant: true },
  );
  expect(seen).toHaveLength(2);
  // 조용한 재구성에서도 지나간 일이라 넘기지 않는다
  layer.sync(
    withCues(office([team("t", [boss()])]), [cue({ kind: "Return" })], true),
    "large",
    NOW + 30,
  );
  expect(seen).toHaveLength(2);
});

test("땀방울: 쪼그려 앉은 지 2분이 넘어야 보이고 일어서면 사라진다. 움직임 줄이기면 첫 장", () => {
  const sheet = {
    data: officeJson,
    textures: Object.fromEntries(
      Object.keys(officeJson.frames).map((k) => [k, new Texture({ label: k })]),
    ),
  } as unknown as Spritesheet;
  const layer = new ActorLayer(fp, sheet, owls, document.createElement("div"));
  layer.attach(new Container());
  const squat = team("t", [boss(), emp("a", "Build", "Squat")]);
  layer.sync(office([squat]), "large", NOW);
  const sweat = () =>
    layer
      .rootOf("t", "a")!
      .children.find((c) => (c as Sprite).texture?.label?.startsWith("sweat/")) as
      | Sprite
      | undefined;
  layer.update(NOW);
  expect(sweat()?.visible ?? false).toBe(false);
  // 120_000 은 "넘어야" 라서 정확히 2분은 아직
  layer.update(Date.parse(SINCE) + 120_000);
  expect(sweat()?.visible ?? false).toBe(false);
  layer.update(Date.parse(SINCE) + 120_001);
  expect(sweat()!.visible).toBe(true);
  const frames = [0, 125, 250, 375].map((ms) => {
    layer.update(Date.parse(SINCE) + 130_000 + ms);
    return sweat()!.texture.label;
  });
  expect(new Set(frames)).toEqual(new Set(["sweat/0", "sweat/1"]));
  vi.stubGlobal("matchMedia", () => ({ matches: true }));
  try {
    layer.update(Date.parse(SINCE) + 130_300);
    expect(sweat()!.texture.label).toBe("sweat/0");
  } finally {
    vi.unstubAllGlobals();
  }
  layer.sync(office([team("t", [boss(), emp("a", "Build", "Sit")])]), "large", NOW + 100_000);
  layer.update(Date.parse(SINCE) + 131_000);
  expect(sweat()!.visible).toBe(false);
});

// ---------- 미팅실·외근 (T26) ----------

test("미팅실: MCP 는 서서, 스킬은 앉아서 (둘 다 Meeting·Sit 이라 meeting 값으로 가른다)", () => {
  const look = (meeting: EmployeeDto["meeting"]) =>
    at("large", office([team("t", [boss(), emp("a", "Meeting", "Sit", { meeting })])]), "a")!.look;
  expect(look({ kind: "Mcp", server: "figma" })).toEqual({ pose: "idle", dir: "f" });
  expect(look({ kind: "Skill" })).toEqual({ pose: "sit", dir: "f" });
});

// 외근: 숨었다 돌아오는 직원이 있는 층
function awayLayer(o: { owls?: Owls | null } = {}) {
  const sheet = {
    data: officeJson,
    textures: Object.fromEntries(
      Object.keys(officeJson.frames).map((k) => [k, new Texture({ label: k })]),
    ),
  } as unknown as Spritesheet;
  const layer = new ActorLayer(fp, sheet, o.owls ?? labeled, document.createElement("div"));
  layer.attach(new Container());
  const doors: number[] = [];
  layer.onDoor = (now) => void doors.push(now);
  const a = emp("a", "Dev", "Type", { seat: 0 });
  const out = { ...a, activity: { ...a.activity, room: "Outside" as Room } };
  const held = () =>
    layer
      .rootOf("t", "a")!
      .children.some((c) => (c as Sprite).texture?.label === "held_envelope/default");
  return { layer, doors, held, a, out };
}

test("외근 복귀: 문이 열리고 문에서 봉투를 들고 걸어 자리로 간다", () => {
  const { layer, doors, held, a, out } = awayLayer();
  layer.sync(office([team("t", [boss(), a])]), "large", NOW);
  layer.sync(office([team("t", [boss(), out])]), "large", NOW);
  layer.update(NOW + WALK_MAX_MS);
  expect(layer.rootOf("t", "a")!.visible).toBe(false);
  expect(doors).toEqual([]);

  layer.sync(office([team("t", [boss(), a])]), "large", NOW + 3000);
  expect(doors).toEqual([NOW + 3000]);
  expect(posOf(layer, "a")).toEqual(screen(...DOOR));
  expect(bodyOf(layer, "a")).toMatch(/^body\/\w+\/walk_/);
  expect(held()).toBe(true);
  // 자리에 앉으면 봉투를 내려놓는다
  layer.update(NOW + 3000 + WALK_MAX_MS);
  expect(posOf(layer, "a")).toEqual(SEAT0);
  expect(held()).toBe(false);
});

test("외근 복귀가 아닌 출근(Enter)·조용한 재구성(instant)·움직임 줄이기에는 문도 봉투도 없다", () => {
  const back = (run: (l: ActorLayer, o: OfficeDto) => void) => {
    const { layer, doors, held, a, out } = awayLayer();
    layer.sync(office([team("t", [boss(), a])]), "large", NOW);
    layer.sync(office([team("t", [boss(), out])]), "large", NOW);
    layer.update(NOW + WALK_MAX_MS);
    run(layer, office([team("t", [boss(), a])]));
    return [doors.length, held()];
  };
  expect(
    back((l, o) => l.sync(withCues(o, [cue({ kind: "Enter", agent: "a" })]), "large", NOW + 3000)),
  ).toEqual([0, false]);
  expect(back((l, o) => l.sync(o, "large", NOW + 3000, { instant: true }))).toEqual([0, false]);
  vi.stubGlobal("matchMedia", () => ({ matches: true }));
  try {
    expect(back((l, o) => l.sync(o, "large", NOW + 3000))).toEqual([0, false]);
  } finally {
    vi.unstubAllGlobals();
  }
  // 대조: 보통은 문이 열린다
  expect(back((l, o) => l.sync(o, "large", NOW + 3000))).toEqual([1, true]);
});

// ---------- 서브 종료·액자 교체 (T26b) ----------

// 액자 교체: 제 자리에 앉은 직원 한 명이 액자 앞 사다리에 4초 서 있다 돌아온다
function frameSwapRun() {
  const { layer } = newLayer();
  const a = emp("a", "Dev", "Type", { seat: 0 });
  const o = () => office([team("t", [boss(), a])]);
  layer.sync(o(), "large", NOW);
  const ladders: number[] = [];
  layer.onLadder = (start) => void ladders.push(start);
  return { layer, o, ladders };
}
const swap = (title = "새 제목") => cue({ kind: "FrameSwap", title });
const frameAt = () => {
  const f = fp.layouts.large.placements.find((p) => p.id === "rules_frame")!;
  return screen(f.gx + 0.8, f.gy + 1.2);
};

test("FrameSwap: 제 자리 직원 한 명이 액자 앞까지 걸어가 LADDER_MS 서 있다 자리로 돌아온다", () => {
  const { layer, o, ladders } = frameSwapRun();
  const home = ["boss", "a"].map((id) => posOf(layer, id));
  layer.sync(withCues(o(), [swap()]), "large", NOW + 100);
  const t = NOW + 100;
  // 사다리 시각 = 걸어서 도착하는 시각
  expect(ladders).toHaveLength(1);
  expect(ladders[0]).toBeGreaterThanOrEqual(t + 500);
  expect(ladders[0]).toBeLessThanOrEqual(t + WALK_MAX_MS);
  const where = (ms: number) => {
    layer.update(ms);
    return ["boss", "a"].map((id) => posOf(layer, id));
  };
  const front = where(ladders[0] + 10).filter((p) => p.x === frameAt().x && p.y === frameAt().y);
  expect(front).toHaveLength(1);
  expect(where(ladders[0] + LADDER_MS - 10).filter((p) => p.x === frameAt().x)).toHaveLength(1);
  expect(where(ladders[0] + LADDER_MS + WALK_MAX_MS)).toEqual(home);
});

test("FrameSwap: instant·움직임 줄이기·앉은 직원이 없으면 가지 않고, 같은 스냅샷을 다시 불러도 한 번이다", () => {
  const { layer, o, ladders } = frameSwapRun();
  layer.sync(withCues(o(), [swap()]), "large", NOW + 100, { instant: true });
  expect(ladders).toHaveLength(0);
  const cued = withCues(o(), [swap()]);
  layer.sync(cued, "large", NOW + 200);
  layer.sync(cued, "large", NOW + 210);
  expect(ladders).toHaveLength(1);
  // 탕비실에 가 있는 직원과 퇴장 서브는 제 자리에 있지 않다
  const away = newLayer();
  const lazy = emp("a", "Pantry", "Sit", { retiredAt: SINCE });
  away.layer.sync(withCues(office([team("t", [lazy])]), [swap()]), "large", NOW);
  const seen: number[] = [];
  away.layer.onLadder = (s) => void seen.push(s);
  away.layer.sync(withCues(office([team("t", [lazy])]), [swap("또")]), "large", NOW + 10);
  expect(seen).toHaveLength(0);
  vi.stubGlobal("matchMedia", () => ({ matches: true }));
  try {
    const still = frameSwapRun();
    still.layer.sync(withCues(still.o(), [swap()]), "large", NOW + 100);
    expect(still.ladders).toHaveLength(0);
    expect(posOf(still.layer, "a")).toEqual(SEAT0);
  } finally {
    vi.unstubAllGlobals();
  }
});

test("FrameSwap: 한 sync 에 여럿 와도 액자는 하나라 마지막 것만 연출한다", () => {
  const { layer, o, ladders } = frameSwapRun();
  layer.sync(withCues(o(), [swap("가"), swap("나")]), "large", NOW + 100);
  expect(ladders).toHaveLength(1);
});

test("FrameSwap: 가는 사람은 늘 한 명이다", () => {
  const { layer } = newLayer();
  const emps = [0, 1, 2, 3].map((i) => emp(`e${i}`, "Dev", "Type", { seat: i }));
  const o = () => office([team("t", [boss(), ...emps])]);
  layer.sync(o(), "large", NOW);
  layer.sync(withCues(o(), [swap("가")]), "large", NOW + 100);
  layer.update(NOW + 100 + 400);
  const all = [boss(), ...emps];
  const moved = all.filter((e) => posOf(layer, e.agentId).x !== at("large", o(), e.agentId)!.x);
  expect(moved).toHaveLength(1);
});

test("회의실은 계획 진행 자리이며 만석은 겹치지 않고 배지로 표시한다", () => {
  const o = office([
    team("t", [boss("Conference"), ...[0, 1, 2, 3].map((i) => emp(`p${i}`, "Conference"))]),
  ]);
  const crowd = new Map<string, number>();
  const placed = layoutActors(fp, "large", o, crowd);
  expect(placed.map((p) => p.spot)).toEqual([
    "conference_0",
    "conference_1",
    "conference_2",
    "conference_3",
  ]);
  expect(crowd).toEqual(new Map([["conference_3", 1]]));
  const { layer } = newLayer();
  layer.sync(o, "large", NOW);
  layer.sync(office([team("t", [boss("Incident")])]), "large", NOW + 1);
  layer.update(NOW + 1 + WALK_MAX_MS);
  expect(posOf(layer, "boss")).toEqual(screen(...(fp.stands.incident[0] as [number, number])));
  layer.sync(office([team("t", [boss()])]), "large", NOW + 2000);
  layer.update(NOW + 2000 + WALK_MAX_MS);
  expect(posOf(layer, "boss")).toEqual(
    screen(...(fp.layouts.large.seats.boss.map((p) => [p.gx, p.gy])[0] as [number, number])),
  );
});

test("S58 활동 팀의 종료 서브와 살아 있는 졸음 팀이 탕비실·옥상을 나누고 복귀한다", () => {
  const teams = [
    testTeam(
      "working",
      [
        testEmp("working-boss", {
          rank: "Bujang",
          depth: 0,
          activity: { ...testEmp("x").activity, room: "TeamLead" },
        }),
        ...Array.from({ length: 5 }, (_, j) =>
          testEmp(`done${j}`, {
            parent: "working-boss",
            retiredAt: SINCE,
            activity: { ...testEmp("x").activity, room: "Pantry", pose: "Sit" },
          }),
        ),
      ],
      { deskSlot: 0 },
    ),
    ...Array.from({ length: 5 }, (_, i) =>
      testTeam(
        `rest${i}`,
        [
          testEmp(`boss${i}`, {
            rank: "Bujang",
            depth: 0,
            activity: { ...testEmp("x").activity, room: "Pantry", pose: "Sleep" },
          }),
        ],
        { deskSlot: i + 1, phase: "Dozing", idleAlive: true },
      ),
    ),
  ];
  const o = testOffice("vendor/Claude", 1, teams);
  const crowd = new Map<string, number>();
  const placed = layoutActors(fp, "small", o, crowd);
  expect(placed).toHaveLength(10);
  const resting = placed.filter((p) => p.emp.activity.room === "Pantry");
  expect(resting).toHaveLength(9);
  expect(new Set(resting.map((p) => p.spot)).size).toBe(9);
  expect(resting.filter((p) => p.spot.startsWith("roof_"))).toHaveLength(3);
  expect(resting.filter((p) => p.team.teamId === "working")).toHaveLength(4);
  expect(crowd).toEqual(new Map([["pantry_1", 1]]));
  const fewer = { ...o, teams: teams.slice(1) };
  expect(layoutActors(fp, "small", fewer).some((p) => p.spot.startsWith("roof_"))).toBe(false);
  const { layer, overlay } = newLayer();
  layer.sync(o, "small", NOW);
  expect(overlay.querySelector(".ow-crowd")?.textContent).toBe("+1");
  layer
    .rootOf("rest2", "boss2")!
    .emit("pointerover", new FederatedPointerEvent(new EventBoundary()));
  expect(overlay.querySelector(".ow-nametag span")?.textContent).toContain("옥상 휴식 (추정)");
  layer.sync(fewer, "small", NOW + 1);
  layer.update(NOW + 1 + WALK_MAX_MS);
  const target = layoutActors(fp, "small", fewer).find((p) => p.emp.agentId === "boss2")!;
  expect(target.spot.startsWith("pantry")).toBe(true);
  expect(posOf(layer, "boss2", "rest2")).toEqual({ x: target.x, y: target.y });
  const active = {
    ...o,
    teams: teams.map((t) =>
      t.teamId === "rest2"
        ? testTeam(
            "rest2",
            [
              testEmp("boss2", {
                rank: "Bujang",
                depth: 0,
                activity: { ...testEmp("x").activity, room: "TeamLead" },
              }),
            ],
            { deskSlot: 3 },
          )
        : t,
    ),
  };
  const home = layoutActors(fp, "small", active).find((p) => p.emp.agentId === "boss2")!;
  expect(home.spot).toBe("boss_3");
  layer.sync(o, "small", NOW + 2000);
  layer.update(NOW + 2000 + WALK_MAX_MS);
  const roof = placed.find((p) => p.emp.agentId === "boss2")!;
  expect(roof.spot.startsWith("roof_")).toBe(true);
  expect(posOf(layer, "boss2", "rest2")).toEqual({ x: roof.x, y: roof.y });
  layer.sync(active, "small", NOW + 4000);
  layer.update(NOW + 4000 + WALK_MAX_MS);
  expect(posOf(layer, "boss2", "rest2")).toEqual({ x: home.x, y: home.y });
  const off = {
    ...o,
    teams: teams.map((t) => (t.teamId === "rest2" ? { ...t, phase: "Off" as const } : t)),
  };
  expect(layoutActors(fp, "small", off).some((p) => p.emp.agentId === "boss2")).toBe(false);
});

for (const [kind, room, ms] of [
  ["Compaction", "archive", 4000],
  ["Reject", "security", 3000],
] as const) {
  const event = cue(
    kind === "Compaction" ? { kind, agent: "boss", at: SINCE } : { kind, agent: "boss" },
  );
  test(`${room}: 새 사건만 방문하고 기한에 현재 자리로 복귀한다`, () => {
    const { layer } = newLayer();
    const archive = vi.fn();
    layer.onArchiveVisit = archive;
    layer.sync(office([team("t", [boss()])]), "large", NOW);
    const o = withCues(office([team("t", [boss()])]), [event]);
    layer.sync(o, "large", NOW);
    layer.update(NOW + WALK_MAX_MS);
    expect(posOf(layer, "boss")).toEqual(screen(...(fp.stands[room][0] as [number, number])));
    layer.sync(o, "large", NOW + ms - 1);
    layer.update(NOW + ms);
    const home = layoutActors(fp, "large", o)[0];
    expect(posOf(layer, "boss")).toEqual({ x: home.x, y: home.y });
    expect(archive).toHaveBeenLastCalledWith(false);
    layer.update(NOW + ms + WALK_MAX_MS);
    expect(posOf(layer, "boss")).toEqual({ x: home.x, y: home.y });
  });
  test(`${room}: 높은 활동·숨김·재구성·전환은 방문과 파쇄기를 끊고 지난 사건을 재생하지 않는다`, () => {
    for (const stop of [
      "pending",
      "permission",
      "incident",
      "off",
      "hidden",
      "rebuilt",
      "switch",
    ] as const) {
      const { layer } = newLayer();
      const archive = vi.fn();
      layer.onArchiveVisit = archive;
      layer.sync(office([team("t", [boss()])]), "large", NOW);
      layer.sync(withCues(office([team("t", [boss()])]), [event]), "large", NOW);
      const b = boss(
        stop === "permission" ? "Door" : stop === "incident" ? "Incident" : "TeamLead",
      );
      if (stop === "pending") b.pendingCount = 1;
      const t = team("t", [b], stop === "off" ? { phase: "Off" } : {});
      if (stop === "incident") t.incident = { since: SINCE, lastError: SINCE };
      const o = withCues(office([t]), [], stop === "rebuilt");
      if (stop === "switch") o.officeId = "other";
      layer.sync(o, "large", NOW + 100, { instant: stop === "hidden" });
      expect(archive, stop).toHaveBeenLastCalledWith(false);
      layer.update(NOW + 100 + WALK_MAX_MS);
      const home = layoutActors(fp, "large", o)[0];
      if (home) expect(posOf(layer, "boss"), stop).toEqual({ x: home.x, y: home.y });
      else expect(layer.rootOf("t", "boss")?.visible).toBe(false);
      layer.sync(office([team("t", [boss()])]), "large", NOW + 2000);
      expect(archive, stop).toHaveBeenLastCalledWith(false);
    }
  });
  test(`${room}: 동시 방문은 두 자리와 만석 배지로 제한한다`, () => {
    const { layer, overlay } = newLayer();
    const employees = [
      boss(),
      emp("a", "Dev", "Sit", { seat: 0 }),
      emp("b", "Dev", "Sit", { seat: 1 }),
    ];
    layer.sync(office([team("t", employees)]), "large", NOW);
    const events = employees.map((e) =>
      cue(
        kind === "Compaction" ? { kind, agent: e.agentId, at: SINCE } : { kind, agent: e.agentId },
      ),
    );
    const placed = layer.sync(withCues(office([team("t", employees)]), events), "large", NOW);
    expect(placed.map((p) => p.spot)).toEqual([`${room}_0`, `${room}_1`]);
    expect(overlay.querySelector(".ow-crowd")?.textContent).toBe("+1");
  });
}

test("반려 뒤 1초 압축은 생략하고 반려 유지가 끝난 4초 뒤 새 압축만 허용한다", () => {
  const { layer } = newLayer();
  const held = { ...boss(), rejected: true };
  layer.sync(office([team("t", [boss()])]), "large", NOW);
  layer.sync(
    withCues(office([team("t", [held])]), [cue({ kind: "Reject", agent: "boss" })]),
    "large",
    NOW,
  );
  const compact = { ...held, activity: { ...held.activity, sentence: "분갈이 중" } };
  const skipped = layer.sync(
    withCues(office([team("t", [compact])]), [
      cue({ kind: "Compaction", agent: "boss", at: SINCE }),
    ]),
    "large",
    NOW + 1000,
  );
  expect(skipped[0].spot).toBe("security_0");
  layer.update(NOW + 3000);
  expect(posOf(layer, "boss")).toEqual(
    screen(fp.layouts.large.seats.boss[0].gx, fp.layouts.large.seats.boss[0].gy),
  );
  const fresh = {
    ...compact,
    rejected: false,
    activity: { ...compact.activity, sentence: "반려: 옛 호출" },
  };
  const allowed = layer.sync(
    withCues(office([team("t", [fresh])]), [cue({ kind: "Compaction", agent: "boss", at: SINCE })]),
    "large",
    NOW + 4000,
  );
  expect(allowed[0].spot).toBe("archive_0");
});

test("사무실 id가 달라진 스냅샷의 새 방문 cue도 재생하지 않는다", () => {
  const { layer } = newLayer();
  layer.sync(office([team("t", [boss()])]), "large", NOW);
  const o = withCues({ ...office([team("t", [boss()])]), officeId: "other" }, [
    cue({ kind: "Compaction", agent: "boss", at: SINCE }),
  ]);
  expect(layer.sync(o, "large", NOW + 1)[0].spot).toBe("boss_0");
});

const emoteOf = (overlay: HTMLElement) => overlay.querySelector<HTMLElement>(".ow-emote");
const emoteOffice = (e: EmployeeDto, cues: OfficeDto["cues"] = []) =>
  withCues(office([team("t", [e])]), cues);

for (const mark of ["✓", "✕", "!", "?", "…", "♪"]) {
  test(`감정 ${mark}: 새 전이만 1.2초 표시하고 추정은 점선이다`, () => {
    const { layer, overlay } = newLayer();
    const before = boss();
    before.review = { pens: 0, approved: false };
    layer.sync(emoteOffice(before), "large", NOW);
    const after = structuredClone(before);
    const cues: OfficeDto["cues"] = [];
    if (mark === "✓") after.review!.approved = true;
    if (mark === "?") after.activity.room = "Door";
    if (mark === "…") after.activity.pose = "Pace";
    if (mark === "✕") cues.push(cue({ kind: "Reject", agent: "boss" }));
    if (mark === "!") cues.push(cue({ kind: "Shake", agent: "boss" }));
    if (mark === "♪")
      cues.push(
        cue({
          kind: "Stamp",
          agent: "boss",
          at: SINCE,
          passed: 1,
          failed: 0,
          estimated: false,
          confirmedPass: true,
        }),
      );
    const next = emoteOffice(after, cues);
    layer.sync(next, "large", NOW + 1);
    const el = emoteOf(overlay)!;
    expect(el.textContent).toBe(mark);
    expect(el.getAttribute("aria-hidden")).toBe("true");
    expect(el.classList.contains("is-estimate")).toBe(mark === "✓" || mark === "…");
    layer.update(NOW + 1200);
    expect(emoteOf(overlay)).toBe(el);
    layer.update(NOW + 1201);
    expect(emoteOf(overlay)).toBeNull();
    layer.sync(next, "large", NOW + 1300);
    layer.sync(emoteOffice(after), "large", NOW + 1301);
    expect(emoteOf(overlay)).toBeNull();
  });
}

test("감정: 추정 허가는 점선, 미확정 테스트 도장은 생략한다", () => {
  const { layer, overlay } = newLayer();
  layer.sync(emoteOffice(boss()), "large", NOW);
  const ask = boss("Door");
  ask.activity.confidence = "Inferred";
  layer.sync(emoteOffice(ask), "large", NOW + 1);
  expect(emoteOf(overlay)!.classList.contains("is-estimate")).toBe(true);
  layer.update(NOW + 1201);
  for (const [estimated, passed, failed] of [
    [true, 1, 0],
    [false, 0, 1],
  ]) {
    layer.sync(
      emoteOffice(boss(), [
        cue({
          kind: "Stamp",
          agent: "boss",
          at: SINCE,
          passed,
          failed,
          estimated,
          confirmedPass: false,
        }),
      ]),
      "large",
      NOW + 1300,
    );
    expect(emoteOf(overlay)).toBeNull();
  }
});

test("감정: 보이는 말풍선과 충돌하면 버리고 새 말풍선은 진행 중 표식을 지운다", () => {
  const { layer, overlay } = newLayer();
  layer.sync(emoteOffice(boss()), "large", NOW);
  const reject = cue({ kind: "Reject", agent: "boss" });
  layer.sync(
    emoteOffice({ ...boss(), bubble: bub({ kind: "Say", text: "답" }, NOW) }, [reject]),
    "large",
    NOW + 1,
  );
  expect(emoteOf(overlay)).toBeNull();
  layer.update(NOW + 4000);
  layer.sync(emoteOffice(boss()), "large", NOW + 4001);
  expect(emoteOf(overlay)).toBeNull();
  layer.sync(emoteOffice(boss(), [reject]), "large", NOW + 4002);
  expect(emoteOf(overlay)).not.toBeNull();
  layer.sync(
    emoteOffice({ ...boss(), bubble: bub({ kind: "Say", text: "새 답" }, NOW + 4003) }),
    "large",
    NOW + 4003,
  );
  expect(emoteOf(overlay)).toBeNull();
});

for (const stop of [
  "first",
  "switch",
  "instant",
  "rebuilt",
  "reduced",
  "off",
  "outside",
  "removed",
]) {
  test(`감정: ${stop} 경계에서 지우고 지난 표식을 재생하지 않는다`, () => {
    const { layer, overlay } = newLayer();
    const reject = cue({ kind: "Reject", agent: "boss" });
    const approved = boss();
    approved.review = { pens: 0, approved: true };
    if (stop !== "first") {
      layer.sync(emoteOffice(boss()), "large", NOW);
      layer.sync(emoteOffice(boss(), [reject]), "large", NOW + 1);
      expect(emoteOf(overlay)).not.toBeNull();
    }
    const next = emoteOffice(approved, [reject]);
    if (stop === "switch") next.officeId = "other";
    if (stop === "rebuilt") next.rebuilt = true;
    if (stop === "off") next.teams[0].phase = "Off";
    if (stop === "outside") approved.activity.room = "Outside";
    if (stop === "removed") next.teams = [];
    if (stop === "reduced") vi.stubGlobal("matchMedia", () => ({ matches: true }));
    try {
      layer.sync(next, "large", NOW + 2, { instant: stop === "instant" });
      expect(emoteOf(overlay)).toBeNull();
    } finally {
      vi.unstubAllGlobals();
    }
    const settled = emoteOffice(approved);
    settled.officeId = next.officeId;
    layer.sync(settled, "large", NOW + 3);
    expect(emoteOf(overlay)).toBeNull();
  });
}

test("근속: 직원 hiredAt의 1·3·6시간 경계를 update에 반영하고 늦게 고용한 서브는 0단계다", () => {
  const { layer } = newLayer({ owls: labeled });
  const old = boss();
  old.activity.pose = "Type";
  old.hiredAt = new Date(NOW).toISOString();
  const late = emp("late", "Dev", "Sit", {
    seat: 0,
    hiredAt: new Date(NOW + 6 * 3_600_000).toISOString(),
  });
  layer.sync(office([team("t", [old, late], { started: "2020-01-01T00:00:00Z" })]), "large", NOW);
  const root = layer.rootOf("t", "boss")!;
  const frames = () => root.children.filter((c): c is Sprite => c instanceof Sprite);
  for (const [elapsed, stage] of [
    [3_600_000 - 1, 0],
    [3_600_000, 1],
    [3 * 3_600_000 - 1, 1],
    [3 * 3_600_000, 2],
    [6 * 3_600_000 - 1, 2],
    [6 * 3_600_000, 3],
  ]) {
    layer.update(NOW + elapsed);
    const sprites = frames();
    expect(sprites[1].texture.label).toContain(stage ? "bujang-claude-worn/" : "bujang-claude/");
    expect(sprites[2].visible).toBe(stage > 0);
    if (stage) expect(sprites[2].texture.label).toMatch(new RegExp(`^wear/${stage}/`));
    expect(sprites[3].texture.label).toContain("role/backend/");
    expect(sprites.every((s) => fitsWorld(s))).toBe(true);
  }
  const lateSprites = layer.rootOf("t", "late")!.children as Sprite[];
  expect(lateSprites[2].visible).toBe(false);
  expect(lateSprites[1].texture.label).not.toContain("worn");
});

// ---------- 자세 5종 (T97) ----------

const poseTeam = () =>
  team("t", [boss(), emp("a", "Dev", "Sit", { seat: 0 }), emp("b", "Dev", "Sit", { seat: 1 })]);
const passCue = (
  agent: string,
  ms = NOW,
  over: Partial<Extract<OfficeDto["cues"][number], { kind: "Stamp" }>> = {},
) =>
  cue({
    kind: "Stamp",
    agent,
    at: new Date(ms).toISOString(),
    passed: 0,
    failed: 0,
    estimated: false,
    confirmedPass: true,
    ...over,
  });
const poseIs = (layer: ActorLayer, id: string, pose: string) =>
  bodyOf(layer, id)?.includes(`/${pose}_`) ?? false;
function firstPose(layer: ActorLayer, id: string, pose: string, from: number): number {
  for (let now = from; now <= from + 5000; now += 5) {
    layer.update(now);
    if (poseIs(layer, id, pose)) return now;
  }
  throw new Error(`${id}: ${pose} 자세가 없다`);
}

test("think는 기존 추정 Think를 새 정면 2프레임으로만 바꾸고 활동 종료와 함께 끝난다", () => {
  const { layer } = newLayer({ owls: labeled });
  const e = boss("TeamLead", "Think");
  e.activity.confidence = "Inferred";
  layer.sync(office([team("t", [e])]), "large", NOW);
  expect(bodyOf(layer, "boss")).toMatch(/\/think_f_0$/);
  layer.update(NOW + 1000);
  expect(bodyOf(layer, "boss")).toMatch(/\/think_f_1$/);
  expect(e.activity.confidence).toBe("Inferred");
  layer.sync(office([team("t", [boss("TeamLead", "Type")])]), "large", NOW + 1001);
  expect(poseIs(layer, "boss", "type")).toBe(true);
});

test("highfive: Test/Squat 완료 후 home/Sit 두 직원은 큐 길이가 달라도 동시 도착·1초·반전·복귀", () => {
  const { layer } = newLayer({ owls: labeled });
  const t = poseTeam();
  t.employees[1].activity.room = "Test";
  t.employees[1].activity.pose = "Squat";
  layer.sync(office([t]), "large", NOW);
  layer.sync(
    withCues(office([poseTeam()]), [passCue("a"), passCue("b", NOW + 10_000)]),
    "large",
    NOW + 1,
  );
  const start = firstPose(layer, "a", "highfive", NOW + 1);
  expect(poseIs(layer, "b", "highfive")).toBe(true);
  expect(posOf(layer, "a")).toEqual(screen(13, 10.6));
  expect(posOf(layer, "b")).toEqual(screen(13.6, 10));
  expect(layer.rootOf("t", "a")!.scale.x).toBe(1);
  expect(layer.rootOf("t", "b")!.scale.x).toBe(-1);
  layer.update(start + 990);
  expect(poseIs(layer, "a", "highfive")).toBe(true);
  expect(poseIs(layer, "b", "highfive")).toBe(true);
  layer.update(start + 1000);
  expect(poseIs(layer, "a", "highfive")).toBe(false);
  expect(poseIs(layer, "b", "highfive")).toBe(false);
  layer.update(start + 2500);
  expect(posOf(layer, "a")).toEqual(
    screen(fp.layouts.large.seats.dev[0].gx, fp.layouts.large.seats.dev[0].gy),
  );
  expect(posOf(layer, "b")).toEqual(
    screen(fp.layouts.large.seats.dev[1].gx, fp.layouts.large.seats.dev[1].gy),
  );
});

test.each(["late", "same", "estimated", "unconfirmed", "other-team", "compaction"])(
  "highfive: %s 사건은 짝을 만들지 않는다",
  (why) => {
    const { layer } = newLayer({ owls: labeled });
    const t = poseTeam();
    const o = office([t, team("u", [emp("b", "TeamLead", "Sit", { rank: "Bujang" })])]);
    layer.sync(o, "large", NOW);
    const second =
      why === "compaction"
        ? cue({ kind: "Compaction", agent: "b", at: SINCE })
        : passCue(why === "same" ? "a" : "b", NOW + (why === "late" ? 10_001 : 1), {
            estimated: why === "estimated",
            confirmedPass: why !== "unconfirmed",
            ...(why === "other-team" ? { teamId: "u" } : {}),
          });
    layer.sync(withCues({ ...o }, [passCue("a"), second]), "large", NOW + 1);
    for (let dt = 0; dt <= 4000; dt += 100) {
      layer.update(NOW + 1 + dt);
      expect(poseIs(layer, "a", "highfive")).toBe(false);
      expect(poseIs(layer, "b", "highfive")).toBe(false);
    }
  },
);

test("highfive는 두 사건을 소비하고 세 번째 통과와 사용 중 추가 짝을 예약하지 않는다", () => {
  const { layer } = newLayer({ owls: labeled });
  layer.sync(office([poseTeam()]), "large", NOW);
  layer.sync(
    withCues(office([poseTeam()]), [passCue("a"), passCue("b", NOW + 1)]),
    "large",
    NOW + 1,
  );
  firstPose(layer, "a", "highfive", NOW + 1);
  layer.sync(
    withCues(office([poseTeam()]), [passCue("a", NOW + 2), passCue("b", NOW + 3)]),
    "large",
    NOW + 1600,
  );
  layer.update(NOW + 5000);
  layer.sync(withCues(office([poseTeam()]), [passCue("b", NOW + 4)]), "large", NOW + 5001);
  for (let dt = 0; dt <= 3000; dt += 100) {
    layer.update(NOW + 5001 + dt);
    expect(poseIs(layer, "b", "highfive")).toBe(false);
  }
  layer.sync(withCues(office([poseTeam()]), [passCue("a", NOW + 5)]), "large", NOW + 8100);
  firstPose(layer, "a", "highfive", NOW + 8100);
});

test("stretch는 hiredAt 정수 시간 경계에 1.5초, 최초 표시와 복귀의 지난 경계는 버린다", () => {
  const { layer } = newLayer({ owls: labeled });
  const t = poseTeam();
  t.employees[0].hiredAt = new Date(NOW - 3_600_000 + 100).toISOString();
  layer.sync(office([t]), "large", NOW);
  layer.update(NOW + 99);
  expect(poseIs(layer, "boss", "stretch")).toBe(false);
  layer.update(NOW + 100);
  expect(poseIs(layer, "boss", "stretch")).toBe(true);
  layer.update(NOW + 1599);
  expect(poseIs(layer, "boss", "stretch")).toBe(true);
  layer.update(NOW + 1600);
  expect(poseIs(layer, "boss", "stretch")).toBe(false);
  layer.sync(office([t]), "large", NOW + 8 * 3_600_000, { instant: true });
  layer.update(NOW + 8 * 3_600_000 + 1);
  expect(poseIs(layer, "boss", "stretch")).toBe(false);
  const first = newLayer({ owls: labeled }).layer;
  first.sync(office([t]), "large", NOW + 8 * 3_600_000);
  expect(poseIs(first, "boss", "stretch")).toBe(false);
});

test("drink는 Pace 진입 시각 대신 실제 activity.since의 30초 경계를 사용한다", () => {
  const { layer } = newLayer({ owls: labeled });
  layer.sync(office([team("t", [boss()])]), "large", NOW);
  const e = boss("TeamLead", "Pace");
  e.activity.since = new Date(NOW - 29_000).toISOString();
  layer.sync(office([team("t", [e])]), "large", NOW + 1);
  layer.update(NOW + 999);
  expect(poseIs(layer, "boss", "drink")).toBe(false);
  const start = firstPose(layer, "boss", "drink", NOW + 1000);
  expect(start).toBeLessThan(NOW + 3000);
  layer.update(start + 990);
  expect(poseIs(layer, "boss", "drink")).toBe(true);
  layer.update(start + 1000);
  expect(poseIs(layer, "boss", "drink")).toBe(false);
});

test("phone는 부장의 기존 jump 0.4초 뒤 1초만 이어진다", () => {
  const { layer } = newLayer({ owls: labeled });
  layer.sync(office(BOSS_IN), "large", NOW);
  layer.sync(withCues(office(BOSS_IN), [visitCue()]), "large", NOW + 1);
  layer.update(NOW + 400);
  expect(poseIs(layer, "boss", "jump")).toBe(true);
  layer.update(NOW + 401);
  expect(poseIs(layer, "boss", "phone")).toBe(true);
  layer.update(NOW + 1400);
  expect(poseIs(layer, "boss", "phone")).toBe(true);
  layer.update(NOW + 1401);
  expect(poseIs(layer, "boss", "phone")).toBe(false);
});

test.each([
  "pending",
  "door",
  "rejected",
  "incident",
  "off",
  "retired",
  "departed",
  "outside",
  "planning",
  "work",
  "sleep",
  "pace",
  "meeting",
  "visit",
])("stretch·highfive·phone는 %s 활동을 덮지 않고 지난 시간 경계도 재생하지 않는다", (stop) => {
  const { layer } = newLayer({ owls: labeled });
  const t = poseTeam();
  const e = t.employees[0];
  e.hiredAt = new Date(NOW - 3_600_000 + 100).toISOString();
  if (stop === "pending") e.pendingCount = 1;
  if (stop === "door") e.activity.room = "Door";
  if (stop === "rejected") e.rejected = true;
  if (stop === "incident") t.incident = { since: SINCE, lastError: "error" };
  if (stop === "off") t.phase = "Off";
  if (stop === "retired") e.retiredAt = SINCE;
  if (stop === "departed") e.departedAt = SINCE;
  if (stop === "outside") e.activity.room = "Outside";
  if (stop === "planning") e.planning = true;
  if (stop === "work") e.activity.pose = "Type";
  if (stop === "sleep") e.activity.pose = "Sleep";
  if (stop === "pace") e.activity.pose = "Pace";
  if (stop === "meeting") e.activity.room = "Meeting";
  layer.sync(office([t]), "large", NOW);
  layer.sync(
    withCues(office([t]), [
      ...(stop === "visit" ? [cue({ kind: "Compaction", agent: "boss", at: SINCE })] : []),
      passCue("boss"),
      passCue("a", NOW + 1),
      visitCue(),
    ]),
    "large",
    NOW + 1,
  );
  for (let dt = 100; dt < 3000; dt += 100) {
    layer.update(NOW + dt);
    const root = layer.rootOf("t", "boss");
    if (root?.visible)
      for (const pose of ["stretch", "highfive", "phone"])
        expect(poseIs(layer, "boss", pose)).toBe(false);
  }
  const resumed = poseTeam();
  resumed.employees[0].hiredAt = e.hiredAt;
  layer.sync(office([resumed]), "large", NOW + 4000);
  layer.update(NOW + 4500);
  expect(poseIs(layer, "boss", "stretch")).toBe(false);
  expect(poseIs(layer, "boss", "highfive")).toBe(false);
});

test("응답 대기에서는 stretch가 없고 장애 중 drink 경계는 소비한다", () => {
  const { layer } = newLayer({ owls: labeled });
  const e = boss("TeamLead", "Pace");
  e.hiredAt = new Date(NOW - 3_600_000 + 100).toISOString();
  e.activity.since = new Date(NOW - 30_000 + 100).toISOString();
  const t = team("t", [e], { incident: { since: SINCE, lastError: "error" } });
  layer.sync(office([t]), "large", NOW);
  for (let dt = 100; dt <= 2000; dt += 100) {
    layer.update(NOW + dt);
    expect(poseIs(layer, "boss", "stretch")).toBe(false);
    expect(poseIs(layer, "boss", "drink")).toBe(false);
  }
  layer.sync(office([{ ...t, incident: null }]), "large", NOW + 2001);
  layer.update(NOW + 2500);
  expect(poseIs(layer, "boss", "drink")).toBe(false);
});

test.each(["stretch", "drink", "phone", "highfive"])(
  "%s 도중 같은 자리 pending이 생기면 즉시 끊고 상대도 함께 끊는다",
  (pose) => {
    const { layer } = newLayer({ owls: labeled });
    const t = poseTeam();
    t.employees[0].hiredAt = new Date(NOW - 3_600_000 + 100).toISOString();
    if (pose === "drink") {
      t.employees[0].activity.pose = "Pace";
      t.employees[0].activity.since = new Date(NOW - 30_000 + 100).toISOString();
    }
    layer.sync(office([t]), "large", NOW);
    layer.sync(
      withCues(
        office([t]),
        pose === "phone"
          ? [visitCue()]
          : pose === "highfive"
            ? [passCue("boss"), passCue("a", NOW + 1)]
            : [],
      ),
      "large",
      NOW + 1,
    );
    const start = firstPose(layer, "boss", pose, NOW + 2);
    const changed = {
      ...t,
      employees: t.employees.map((e) => (e.agentId === "boss" ? { ...e, pendingCount: 1 } : e)),
    };
    layer.sync(office([changed]), "large", start + 1);
    expect(poseIs(layer, "boss", pose)).toBe(false);
    if (pose === "highfive") expect(poseIs(layer, "a", pose)).toBe(false);
    layer.update(start + 3000);
    expect(poseIs(layer, "boss", pose)).toBe(false);
  },
);

test.each(["switch", "instant", "rebuilt", "reduced"])(
  "%s 경계에서 하이파이브 대기 사건·활성 자세·타이머를 소비한다",
  (stop) => {
    const { layer } = newLayer({ owls: labeled });
    const t = poseTeam();
    t.employees[0].hiredAt = new Date(NOW - 3_600_000 + 100).toISOString();
    layer.sync(office([t]), "large", NOW);
    layer.sync(withCues(office([t]), [passCue("a")]), "large", NOW + 1);
    layer.update(NOW + 100);
    expect(poseIs(layer, "boss", "stretch")).toBe(true);
    const next = withCues(office([t]), [passCue("b", NOW + 1)]);
    if (stop === "switch") next.officeId = "other";
    if (stop === "rebuilt") next.rebuilt = true;
    if (stop === "reduced") vi.stubGlobal("matchMedia", () => ({ matches: true }));
    try {
      layer.sync(next, "large", NOW + 200, { instant: stop === "instant" });
      expect(poseIs(layer, "boss", "stretch")).toBe(false);
      layer.update(NOW + 3_600_100);
    } finally {
      vi.unstubAllGlobals();
    }
    const resumed = office([t]);
    resumed.officeId = next.officeId;
    layer.sync(resumed, "large", NOW + 3_600_101, { instant: true });
    for (let dt = 0; dt <= 3000; dt += 100) {
      layer.update(NOW + 3_600_102 + dt);
      expect(poseIs(layer, "boss", "stretch")).toBe(false);
      expect(poseIs(layer, "a", "highfive")).toBe(false);
      expect(poseIs(layer, "b", "highfive")).toBe(false);
    }
  },
);

test("한 번 소비한 확정 통과 사건을 새 스냅샷에 반복해도 재사용하지 않는다", () => {
  const { layer } = newLayer({ owls: labeled });
  const cues = [passCue("a"), passCue("b", NOW + 1)];
  layer.sync(office([poseTeam()]), "large", NOW);
  layer.sync(withCues(office([poseTeam()]), cues), "large", NOW + 1);
  firstPose(layer, "a", "highfive", NOW + 1);
  layer.update(NOW + 5000);
  layer.sync(withCues(office([poseTeam()]), cues), "large", NOW + 5001);
  for (let dt = 0; dt <= 3000; dt += 100) {
    layer.update(NOW + 5001 + dt);
    expect(poseIs(layer, "a", "highfive")).toBe(false);
  }
});

test.each(["switch", "instant", "rebuilt", "reduced"])(
  "진행 중 highfive는 %s에서 둘 다 중단하고 다시 재생하지 않는다",
  (stop) => {
    const { layer } = newLayer({ owls: labeled });
    layer.sync(office([poseTeam()]), "large", NOW);
    layer.sync(
      withCues(office([poseTeam()]), [passCue("a"), passCue("b", NOW + 1)]),
      "large",
      NOW + 1,
    );
    const start = firstPose(layer, "a", "highfive", NOW + 1);
    const next = office([poseTeam()]);
    if (stop === "switch") next.officeId = "other";
    if (stop === "rebuilt") next.rebuilt = true;
    if (stop === "reduced") vi.stubGlobal("matchMedia", () => ({ matches: true }));
    try {
      if (stop === "reduced") layer.update(start + 1);
      else layer.sync(next, "large", start + 1, { instant: stop === "instant" });
      expect(poseIs(layer, "a", "highfive")).toBe(false);
      expect(poseIs(layer, "b", "highfive")).toBe(false);
    } finally {
      vi.unstubAllGlobals();
    }
    layer.update(start + 3000);
    expect(poseIs(layer, "a", "highfive")).toBe(false);
    expect(poseIs(layer, "b", "highfive")).toBe(false);
  },
);

test("하이파이브 장소는 사무실 공유라 첫 팀만 연출하고 그동안 생긴 다른 팀 짝은 소비한다", () => {
  const { layer } = newLayer({ owls: labeled });
  const t = poseTeam();
  const u = team("u", [
    emp("lead", "TeamLead", "Sit", { rank: "Bujang" }),
    emp("c", "Dev", "Sit", { seat: 2 }),
  ]);
  layer.sync(office([t, u]), "large", NOW);
  const other = (agent: string, ms: number) => passCue(agent, ms, { teamId: "u" });
  layer.sync(
    withCues(office([t, u]), [
      passCue("a"),
      passCue("b", NOW + 1),
      other("lead", NOW + 2),
      other("c", NOW + 3),
    ]),
    "large",
    NOW + 1,
  );
  firstPose(layer, "a", "highfive", NOW + 1);
  expect(bodyOf(layer, "lead", "u")).not.toContain("/highfive_");
  expect(bodyOf(layer, "c", "u")).not.toContain("/highfive_");
  layer.update(NOW + 5000);
  layer.sync(withCues(office([t, u]), [other("c", NOW + 4)]), "large", NOW + 5001);
  for (let dt = 0; dt <= 3000; dt += 100) {
    layer.update(NOW + 5001 + dt);
    expect(bodyOf(layer, "lead", "u")).not.toContain("/highfive_");
    expect(bodyOf(layer, "c", "u")).not.toContain("/highfive_");
  }
});

function itemLayer() {
  const overlay = document.createElement("div");
  const sheet = {
    data: officeJson,
    textures: Object.fromEntries(
      Object.keys(officeJson.frames).map((k) => [k, new Texture({ label: k })]),
    ),
  } as unknown as Spritesheet;
  const layer = new ActorLayer(fp, sheet, labeled, overlay);
  layer.attach(new Container());
  return layer;
}
const visibleTextures = (layer: ActorLayer, id = "boss") =>
  layer
    .rootOf("t", id)
    ?.children.filter((c): c is Sprite => c instanceof Sprite && c.visible)
    .map((s) => s.texture.label ?? "") ?? [];
const heldItems = (layer: ActorLayer, id = "boss") =>
  visibleTextures(layer, id).filter((k) => k.startsWith("held_"));

for (const item of ["Medal", "Helmet", "Blanket", "Headphones"] as const) {
  test(`착용 ${item}: 새 후보를 자세에 겹치고 후보 종료·퇴근이면 즉시 지운다`, () => {
    const layer = itemLayer();
    layer.sync(office([team("t", [boss()])]), "large", NOW);
    const e = { ...boss(), items: [item] };
    layer.sync(office([team("t", [e])]), "large", NOW + 1);
    expect(visibleTextures(layer).some((k) => k.startsWith(`item/${item.toLowerCase()}/`))).toBe(
      true,
    );
    const step = { ...e, activity: { ...e.activity, room: "Pantry" as const } };
    layer.sync(office([team("t", [step])]), "large", NOW + 2);
    let seenFront = false;
    for (let dt = 0; dt < WALK_MAX_MS; dt += 20) {
      layer.update(NOW + 2 + dt);
      if (bodyOf(layer, "boss")?.includes("/walk_f_")) {
        seenFront = true;
        expect(
          visibleTextures(layer).some((k) => k.startsWith(`item/${item.toLowerCase()}/walk_f_`)),
        ).toBe(true);
      }
    }
    expect(seenFront).toBe(true);
    layer.sync(office([team("t", [boss()])]), "large", NOW + 2000);
    expect(visibleTextures(layer).some((k) => k.startsWith("item/"))).toBe(false);
    layer.sync(office([team("t", [e])]), "large", NOW + 2001);
    layer.sync(office([team("t", [e], { phase: "Off" })]), "large", NOW + 2002);
    expect(visibleTextures(layer).some((k) => k.startsWith("item/"))).toBe(false);
  });
}

test("안전모는 헤드폰·베레모 위, 담요는 근속 수건 위이며 메달을 가린다", () => {
  const layer = itemLayer();
  const e = {
    ...boss(),
    role: "Designer" as const,
    hiredAt: new Date(NOW - 6 * 3_600_000).toISOString(),
    items: ["Medal", "Helmet", "Blanket", "Headphones"] as EmployeeDto["items"],
  };
  layer.sync(office([team("t", [e])]), "large", NOW);
  const keys = visibleTextures(layer);
  expect(keys).toContain("role/designer/sit_f_0");
  expect(keys).toContain("wear/3/sit_f_0");
  expect(keys).toContain("item/helmet/sit_f_0");
  expect(keys).toContain("item/blanket/sit_f_0");
  expect(keys.some((k) => k.startsWith("item/headphones/") || k.startsWith("item/medal/"))).toBe(
    false,
  );
  expect(keys.indexOf("item/blanket/sit_f_0")).toBeGreaterThan(keys.indexOf("wear/3/sit_f_0"));
  expect(keys.indexOf("item/helmet/sit_f_0")).toBeGreaterThan(
    keys.indexOf("role/designer/sit_f_0"),
  );
});

test("두루마리는 계획 중 회의실 읽기·걷기에만 들고 허가 결재판·배송 상자를 덮지 않는다", () => {
  const layer = itemLayer();
  layer.sync(office([team("t", [boss()])]), "large", NOW);
  const e = { ...boss("Conference"), planning: true, items: ["Scroll"] as EmployeeDto["items"] };
  layer.sync(office([team("t", [e])]), "large", NOW + 1);
  expect(heldItems(layer)).toEqual(["held_scroll/default"]);
  layer.update(NOW + 1 + WALK_MAX_MS);
  expect(heldItems(layer)).toEqual(["held_scroll/default"]);
  for (const [room, item] of [
    ["Door", "clipboard"],
    ["Archive", "folder"],
    ["Shipping", "box_carry"],
  ] as const) {
    layer.sync(
      office([team("t", [{ ...e, activity: { ...e.activity, room } }])]),
      "large",
      NOW + 2000,
      { instant: true },
    );
    expect(heldItems(layer)).toEqual([`held_${item}/default`]);
  }
  layer.sync(office([team("t", [boss("Conference")])]), "large", NOW + 2001, { instant: true });
  expect(heldItems(layer)).toEqual([]);
});

for (const leave of ["off", "departed", "outside", "removed"]) {
  test(`퇴장 ${leave}: 가방은 퇴근 걷기에만 들고 완료 후 숨는다`, () => {
    const layer = itemLayer();
    const o = office([team("t", [boss()])]);
    layer.sync(o, "large", NOW);
    const next = office([
      team(
        "t",
        [
          leave === "outside"
            ? boss("Outside")
            : { ...boss(), departedAt: leave === "departed" ? new Date(NOW).toISOString() : null },
        ],
        { phase: leave === "off" ? "Off" : "Working" },
      ),
    ]);
    if (leave === "removed") next.teams = [];
    layer.sync(next, "large", NOW + 1);
    expect(heldItems(layer)).toEqual(
      leave === "off" || leave === "departed" ? ["held_briefcase/default"] : [],
    );
    layer.update(NOW + 1 + WALK_MAX_MS);
    expect(layer.rootOf("t", "boss")?.visible ?? false).toBe(false);
  });
}

for (const previousOff of [true, false]) {
  test(`출입 손전등: 이전 논리 off=${previousOff}, 현재 켜진 사무실로 입장한 동안만`, () => {
    const layer = itemLayer();
    layer.sync(
      office([team("t", [boss()], { phase: previousOff ? "Off" : "Working" })]),
      "large",
      NOW,
    );
    layer.sync(
      withCues(office([team("t", [boss()])]), [cue({ kind: "Enter", agent: "boss" })]),
      "large",
      NOW + 1,
    );
    expect(heldItems(layer)).toEqual(previousOff ? ["held_flashlight/default"] : []);
    layer.update(NOW + ENTER_MS);
    expect(heldItems(layer)).toEqual(previousOff ? ["held_flashlight/default"] : []);
    layer.update(NOW + 1 + ENTER_MS);
    expect(heldItems(layer)).toEqual([]);
  });
}

test("재출근 손전등은 기존 손 물건을 침범하지 않는다", () => {
  for (const room of ["Door", "Archive", "Shipping"] as const) {
    const layer = itemLayer();
    layer.sync(office([team("t", [boss()], { phase: "Off" })]), "large", NOW);
    layer.sync(
      withCues(office([team("t", [boss(room)])]), [cue({ kind: "Rehire", agent: "boss" })]),
      "large",
      NOW + 1,
    );
    expect(heldItems(layer)).not.toContain("held_flashlight/default");
  }
});

for (const quiet of ["first", "instant", "rebuilt", "switch", "reduced"]) {
  test(`출입 ${quiet}: 지난 손전등·가방을 재생하지 않는다`, () => {
    const layer = itemLayer();
    if (quiet !== "first")
      layer.sync(office([team("t", [boss()], { phase: "Off" })]), "large", NOW);
    const next = withCues(
      office([team("t", [boss()])]),
      [cue({ kind: "Enter", agent: "boss" })],
      quiet === "rebuilt",
    );
    if (quiet === "switch") next.officeId = "other";
    if (quiet === "reduced") vi.stubGlobal("matchMedia", () => ({ matches: true }));
    try {
      layer.sync(next, "large", NOW + 1, { instant: quiet === "instant" });
      expect(heldItems(layer)).not.toContain("held_flashlight/default");
      layer.sync(office([team("t", [boss()], { phase: "Off" })]), "large", NOW + 2, {
        instant: true,
      });
      expect(heldItems(layer)).not.toContain("held_briefcase/default");
    } finally {
      vi.unstubAllGlobals();
    }
  });
}

test("출입 중 반대 전이·외근이 생기면 지난 손전등·가방을 즉시 지운다", () => {
  const layer = itemLayer();
  layer.sync(office([team("t", [boss()], { phase: "Off" })]), "large", NOW);
  layer.sync(
    withCues(office([team("t", [boss()])]), [cue({ kind: "Enter", agent: "boss" })]),
    "large",
    NOW + 1,
  );
  expect(heldItems(layer)).toEqual(["held_flashlight/default"]);
  layer.sync(office([team("t", [boss()], { phase: "Off" })]), "large", NOW + 100);
  expect(heldItems(layer)).toEqual(["held_briefcase/default"]);
  layer.sync(office([team("t", [boss("Outside")])]), "large", NOW + 101);
  expect(heldItems(layer)).toEqual([]);
  layer.sync(office([team("t", [boss()])]), "large", NOW + 102, { instant: true });
  expect(heldItems(layer)).toEqual([]);
});

test("퇴근해도 기존 결재판·서류철·박스를 가방으로 덮지 않는다", () => {
  for (const room of ["Door", "Archive", "Shipping"] as const) {
    const layer = itemLayer();
    const o = office([team("t", [boss(room)])]);
    layer.sync(o, "large", NOW);
    const held = heldItems(layer);
    layer.sync(office([team("t", [boss(room)], { phase: "Off" })]), "large", NOW + 1);
    expect(heldItems(layer)).toEqual(held);
  }
});

test("손전등은 진입 걷기 프레임의 손을 따르고 움직임 줄이기로 버린 뒤 재생하지 않는다", () => {
  const layer = itemLayer();
  layer.sync(office([team("t", [boss()], { phase: "Off" })]), "large", NOW);
  layer.sync(
    withCues(office([team("t", [boss()])]), [cue({ kind: "Enter", agent: "boss" })]),
    "large",
    NOW + 1,
  );
  for (const dt of [0, 125, 250, 375]) {
    layer.update(NOW + 1 + dt);
    const name = bodyOf(layer, "boss")!.split("/").pop()!;
    const hand = layer
      .rootOf("t", "boss")!
      .children.find(
        (c): c is Sprite => c instanceof Sprite && c.texture.label === "held_flashlight/default",
      )!;
    expect([hand.x, hand.y]).toEqual([
      (meta.anchors[name].hand[0] - meta.anchor[0]) / meta.density,
      (meta.anchors[name].hand[1] - meta.anchor[1]) / meta.density,
    ]);
  }
  vi.stubGlobal("matchMedia", () => ({ matches: true }));
  try {
    layer.update(NOW + 500);
    expect(heldItems(layer)).toEqual([]);
  } finally {
    vi.unstubAllGlobals();
  }
  layer.update(NOW + 501);
  expect(heldItems(layer)).toEqual([]);
});

test("퇴근한 직원의 새 Rehire도 소등 사무실에서 실제 진입 걷기에만 손전등을 든다", () => {
  const layer = itemLayer();
  layer.sync(office([team("t", [boss()])]), "large", NOW);
  layer.sync(office([team("t", [boss()], { phase: "Off" })]), "large", NOW + 1);
  layer.update(NOW + 1 + WALK_MAX_MS);
  expect(layer.rootOf("t", "boss")!.visible).toBe(false);
  const start = NOW + 2000;
  layer.sync(
    withCues(office([team("t", [boss()])]), [cue({ kind: "Rehire", agent: "boss" })]),
    "large",
    start,
  );
  expect(heldItems(layer)).toEqual(["held_flashlight/default"]);
  layer.update(start + ENTER_MS);
  expect(heldItems(layer)).toEqual([]);
});

for (const [room, pose, name] of [
  ["TeamLead", "Type", "type"],
  ["TeamLead", "Sleep", "sleep"],
  ["TeamLead", "Think", "think"],
  ["Incident", "Sit", "panic"],
] as const)
  test(`움직임 줄이기는 ${name} 몸·옷·담당·근속·착용물을 모두 0프레임에 둔다`, () => {
    let reduced = false;
    vi.stubGlobal("matchMedia", () => ({ matches: reduced }));
    const layer = itemLayer();
    const e = {
      ...boss(room, pose),
      hiredAt: new Date(NOW - 3 * 3_600_000).toISOString(),
      items: ["Medal", "Headphones"] as EmployeeDto["items"],
    };
    try {
      layer.sync(office([team("t", [e])]), "large", NOW);
      const dt = Math.ceil(1000 / meta.poses[name].fps);
      layer.update(NOW + dt);
      expect(bodyOf(layer, "boss")).toMatch(new RegExp(`/${name}_f_1$`));
      reduced = true;
      for (const extra of [1, 501, 2001]) {
        layer.update(NOW + dt + extra);
        const keys = visibleTextures(layer);
        for (const prefix of [
          "body/",
          "outfit/",
          "role/",
          "wear/",
          "item/medal/",
          "item/headphones/",
        ])
          expect(
            keys.some((key) => key.startsWith(prefix)),
            prefix,
          ).toBe(true);
        expect(keys.every((key) => key.endsWith(`/${name}_f_0`))).toBe(true);
      }
    } finally {
      vi.unstubAllGlobals();
    }
  });

test("움직임 줄이기를 켜면 진행 중인 idle 눈 깜빡임도 0프레임에 멈춘다", () => {
  let reduced = false;
  vi.stubGlobal("matchMedia", () => ({ matches: reduced }));
  const random = vi.spyOn(Math, "random").mockReturnValue(0);
  const { layer } = newLayer({ owls: labeled });
  try {
    layer.sync(office([team("t", [boss("Test")])]), "large", NOW);
    layer.update(NOW + 3000);
    expect(bodyOf(layer, "boss")).toMatch(/\/idle_f_1$/);
    reduced = true;
    for (const dt of [3001, 3100, 6000]) {
      layer.update(NOW + dt);
      expect(bodyOf(layer, "boss")).toMatch(/\/idle_f_0$/);
    }
  } finally {
    random.mockRestore();
    vi.unstubAllGlobals();
  }
});

// ---------- 시트 밀도·동물 (HANDOFF 6.3·6.4) ----------

/** 프레임 크기 텍스처로 채운 시트. 레이어가 차지하는 월드 크기를 재려고 */
function sized(json: { frames: object; owlfice: unknown }): Owls {
  const m = json.owlfice as OwlMeta;
  const source = new TextureSource({ width: m.frame[0], height: m.frame[1] });
  const textures = Object.fromEntries(
    Object.keys(json.frames).map((k) => [k, new Texture({ source, label: k })]),
  );
  return { textures, meta: m };
}

test("2× 시트를 3× 로 바꿔도 직원은 같은 자리에서 월드 16×24 를 차지하고 머리 오버레이가 그대로다", () => {
  const two = sized(owlsJson);
  const three = sized(owls3Json);
  const { layer, overlay } = newLayer({ owls: two });
  const a = emp("a", "Dev", "Read", { seat: 0 });
  a.activity.extra = 2;
  layer.sync(office([team("t", [boss(), a])]), "large", NOW);
  layer.placeOverlay((x, y) => toCss(x, y, 3, 1));
  layer.update(NOW);
  const root = layer.rootOf("t", "a")!;
  const badge = overlay.querySelector<HTMLElement>(".ow-parallel")!;
  const seen = () => ({ x: root.x, y: root.y, left: badge.style.left, top: badge.style.top });
  const layers = () => root.children.filter((c): c is Sprite => c instanceof Sprite);
  const size = () => {
    const b = root.getLocalBounds();
    return [b.x, b.width, b.height];
  };
  const before = seen();
  expect(layers().every((s) => fitsWorld(s, two.meta))).toBe(true);
  expect(size()).toEqual([-8, 16, 24]);
  layer.setOwls(three);
  layer.update(NOW + 1);
  expect(layer.rootOf("t", "a")).toBe(root);
  expect(seen()).toEqual(before);
  expect(layers().every((s) => fitsWorld(s, three.meta) && s.scale.x === 1 / 3)).toBe(true);
  const body = layers()[0].texture;
  expect(body).toBe(three.textures[body.label!]);
  expect(size()).toEqual([-8, 16, 24]);
});

test("동물을 바꾸면 직원 종은 새 시트의 employeeSpecies, 사장님 방문 몸체는 bossSpecies 다", () => {
  const dogs = sized(dogsJson);
  const visitorBody = (world: Container, i: number) =>
    ((world.children[i] as Container).children[0] as Sprite).texture.label;
  const { layer, world } = newLayer({ owls: labeled });
  layer.sync(office(BOSS_IN), "large", NOW);
  const base = world.children.length;
  layer.sync(withCues(office(BOSS_IN), [visitCue()]), "large", NOW);
  layer.update(NOW + 1);
  expect(visitorBody(world, base)).toMatch(new RegExp(`^body/${meta.bossSpecies}/`));
  layer.setOwls(dogs);
  layer.update(NOW + 2);
  expect(visitorBody(world, base)).toMatch(new RegExp(`^body/${dogs.meta.bossSpecies}/`));
  const sp = speciesOf("boss", dogs.meta.employeeSpecies);
  expect(bodyOf(layer, "boss")).toMatch(new RegExp(`^body/${sp}/`));
  // 처음부터 강아지 시트면 방문 몸체도 강아지 사장님
  const fresh = newLayer({ owls: dogs });
  fresh.layer.sync(office(BOSS_IN), "large", NOW);
  const n = fresh.world.children.length;
  fresh.layer.sync(withCues(office(BOSS_IN), [visitCue()]), "large", NOW);
  fresh.layer.update(NOW + 1);
  expect(visitorBody(fresh.world, n)).toMatch(new RegExp(`^body/${dogs.meta.bossSpecies}/`));
});
