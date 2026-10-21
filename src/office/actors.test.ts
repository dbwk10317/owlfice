// @vitest-environment jsdom
import { Container, Graphics, Sprite, Texture, type Spritesheet } from "pixi.js";
import { expect, test, vi } from "vite-plus/test";
import rawFloorplan from "../../design/sprites/floorplan.json";
import officeJson from "../../design/sprites/office.json";
import owlsJson from "../../design/sprites/owls.json";
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

const fp = parseFloorplan(rawFloorplan);
const meta = owlsJson.owlfice as unknown as OwlMeta;
const textures = (keys: string[]) =>
  Object.fromEntries(keys.map((k) => [k, Texture.EMPTY])) as Record<string, Texture>;
const owls: Owls = { textures: textures(Object.keys(owlsJson.frames)), meta };
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
    lastSay: null,
    hiredAt: SINCE,
    retiredAt: null,
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
    ...o,
  };
}

function office(teams: TeamDto[]): OfficeDto {
  return {
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
    [boss("TeamLead", "Think"), true, { spot: "boss_0" }, { pose: "idle", dir: "b" }],
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

test("탕비실: 첫째 콘솔 sit, 둘째 안마의자 sleep, 그다음은 서 있고, 넘치면 마지막에 겹친다", () => {
  const ids = ["a", "b", "c", "d", "e"];
  const o = office([
    team("t", [boss(), ...ids.map((id) => emp(id, "Pantry", "Sit", { retiredAt: SINCE }))]),
  ]);
  const got = ids.map((id) => at("large", o, id)!);
  expect(got.map((p) => [p.spot, p.look.pose])).toEqual([
    ["pantry_console_0", "sit"],
    ["pantry_massage_0", "sleep"],
    ["pantry_0", "idle"],
    ["pantry_1", "idle"],
    ["pantry_1", "idle"],
  ]);
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

test("세 겹을 같은 자리에, 수습은 담당 겹을 숨기고 depth 는 zIndexOf 로 배치 뒤", () => {
  const { layer } = newLayer();
  layer.sync(
    office([team("t", [boss(), emp("a", "Dev", "Type", { seat: 0, role: "Trainee" })])]),
    "large",
    NOW,
  );
  const root = layer.rootOf("t", "a")!;
  const sprites = root.children.filter((c) => c instanceof Sprite);
  expect(sprites).toHaveLength(3);
  expect(sprites.map((s) => s.visible)).toEqual([true, true, false]);
  expect(sprites.every((s) => s.pivot.x === 8 && s.pivot.y === 23)).toBe(true);
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
  // idle_f_0 의 head 는 (8, 8): 발 (x, y) 에서 (0, −15)
  const foot = screen(...(fp.stands.test[0] as [number, number]));
  const head = toCss(foot.x, foot.y - 15, 2, 1.25);
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
  // read_f_0 의 head 는 (8, 9): 발 (x, y) 에서 (0, −14), 배지는 거기서 (+14, +6)
  const seat = fp.layouts.large.seats.dev[0];
  const foot = screen(seat.gx, seat.gy);
  expect(badge.style.left).toBe(`${foot.x + 14}px`);
  expect(badge.style.top).toBe(`${foot.y - 14 + 6}px`);
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
  // 방금 시작된 세션의 조용한 재구성: 출근만 연출한다 (PLAN 4.3)
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
  expect(ring).toHaveBeenCalledWith(NOW);
  expect(bodyOf(layer, "boss")).toBe(`body/${sp}/jump_f_0`);
  const visitor = world.children[base] as Container;
  expect((visitor.children[0] as Sprite).texture.label).toMatch(/^body\/snowy\/walk_[fb]_/);
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
  expect(bodyOf(layer, "boss")).toMatch(/\/scratch_f_/);
  expect(stamp.visible).toBe(true);
  layer.update(NOW + STAMP_MS - 1);
  expect(stamp.visible).toBe(true);
  layer.update(NOW + STAMP_MS);
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
  cue({ kind: "Stamp", passed: 12, failed: 0, estimated: false, ...o });
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
