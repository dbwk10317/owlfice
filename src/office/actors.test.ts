// @vitest-environment jsdom
import { Container, Graphics, Sprite, Texture, type Spritesheet } from "pixi.js";
import { expect, test } from "vite-plus/test";
import rawFloorplan from "../../design/sprites/floorplan.json";
import officeJson from "../../design/sprites/office.json";
import owlsJson from "../../design/sprites/owls.json";
import type { EmployeeDto, OfficeDto, Pose, Rank, Role, Room, TeamDto } from "../types/ipc.ts";
import {
  ActorLayer,
  fillElapsed,
  fnv1a64,
  layoutActors,
  owlFrameKeys,
  speciesOf,
  type OwlMeta,
  type Owls,
} from "./actors.ts";
import { gridToScreen, parseFloorplan, type Kind } from "./floorplan.ts";
import { toCss } from "./renderer.ts";

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
  const layer = new ActorLayer(
    fp,
    officeSheet,
    o.owls === undefined ? owls : o.owls,
    overlay,
    () => 0,
  );
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
  const root = layer.rootOf("a")!;
  expect(world.children).toContain(root);
  // 방이 바뀌어도 같은 컨테이너, 위치만 옮긴다
  layer.sync(office([team("t", [boss(), emp("a", "Test")])]), "large", NOW);
  expect(layer.rootOf("a")).toBe(root);
  expect(root.position.x).toBe(screen(...(fp.stands.test[0] as [number, number])).x);
  // 외근은 숨기기만 한다
  layer.sync(office([team("t", [boss(), emp("a", "Outside")])]), "large", NOW);
  expect(layer.rootOf("a")).toBe(root);
  expect(root.visible).toBe(false);
  layer.sync(office([team("t", [boss()])]), "large", NOW);
  expect(layer.rootOf("a")).toBeUndefined();
  expect(root.destroyed).toBe(true);
});

test("세 겹을 같은 자리에, 수습은 담당 겹을 숨기고 depth 는 zIndexOf 로 배치 뒤", () => {
  const { layer } = newLayer();
  layer.sync(
    office([team("t", [boss(), emp("a", "Dev", "Type", { seat: 0, role: "Trainee" })])]),
    "large",
    NOW,
  );
  const root = layer.rootOf("a")!;
  const sprites = root.children.filter((c) => c instanceof Sprite);
  expect(sprites).toHaveLength(3);
  expect(sprites.map((s) => s.visible)).toEqual([true, true, false]);
  expect(sprites.every((s) => s.pivot.x === 8 && s.pivot.y === 23)).toBe(true);
  // 의자(8.7) 뒤, 책상(9.2) 앞
  const n = fp.layouts.large.placements.length;
  expect(root.zIndex).toBe(8900 * 10000 + n + 1);
});

test("왼쪽으로 옮겨 가면 scale.x = −1, 오른쪽으로 오면 1", () => {
  const { layer } = newLayer();
  // 테스트실(x≈350) → 서고(x≈212)
  layer.sync(office([team("t", [boss(), emp("a", "Test")])]), "large", NOW);
  const root = layer.rootOf("a")!;
  expect(root.scale.x).toBe(1);
  layer.sync(office([team("t", [boss(), emp("a", "Archive")])]), "large", NOW);
  expect(root.scale.x).toBe(-1);
  layer.sync(office([team("t", [boss(), emp("a", "Test")])]), "large", NOW);
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
    },
  });
  layer.sync(office([team("t", [boss(), a])]), "large", NOW);
  layer.placeOverlay((x, y) => toCss(x, y, 2, 1.25));
  layer.hover("a");
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
  expect(layer.rootOf("boss")!.children[0]).toBeInstanceOf(Graphics);
});

// ---------- 문장 ----------

test("{elapsed}: 초 → 분 초 → 시간 분, 백틱 안은 그대로", () => {
  const t0 = Date.parse(SINCE);
  expect(fillElapsed("· {elapsed}째", SINCE, t0 + 12_000)).toBe("· 12초째");
  expect(fillElapsed("({elapsed})", SINCE, t0 + 160_000)).toBe("(2분 40초)");
  expect(fillElapsed("자리 비움 {elapsed}", SINCE, t0 + 120_000)).toBe("자리 비움 2분");
  expect(fillElapsed("{elapsed}", SINCE, t0 + 3_900_000)).toBe("1시간 5분");
  // 시계가 어긋나 음수여도 0초
  expect(fillElapsed("{elapsed}", SINCE, t0 - 5_000)).toBe("0초");
  expect(fillElapsed("`echo {elapsed}` · {elapsed}", SINCE, t0 + 1_000)).toBe(
    "`echo {elapsed}` · 1초",
  );
  expect(fillElapsed("그대로", SINCE, t0)).toBe("그대로");
});
