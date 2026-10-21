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
import { CLAP_MS, ENTER_MS, WALK_MAX_MS } from "./anim-queue.ts";
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
  expect(got[4].overflow).toBe(true);
});

test("deskSlot null 팀의 부장은 어디서든 그리지 않는다", () => {
  for (const room of ["TeamLead", "Incident", "Pantry", "Door"] as const)
    expect(
      at("small", office([team("t", [boss(room)], { deskSlot: null })]), "boss"),
      room,
    ).toBeUndefined();
});

test("임시·간이 책상은 쓰는 직원이 있을 때만, 겹쳐 앉으면 만석 쪽지", () => {
  const dev = (id: string, seat: number | null) => emp(id, "Dev", "Sit", { seat });
  const props = (kind: Kind, teams: TeamDto[]) =>
    seatProps(fp, kind, layoutActors(fp, kind, office(teams)));
  expect(props("large", [team("t", [boss(), dev("a", 5)])])).toEqual(new Set());
  expect(props("large", [team("t", [boss(), dev("a", 6), dev("b", 8)])])).toEqual(
    new Set(["temp_desk_0", "temp_desk_2", "note_temp"]),
  );
  // 좌석 번호가 없으면 마지막 임시 책상에 겹친다
  expect(props("large", [team("t", [boss(), dev("a", 9), dev("b", null)])])).toEqual(
    new Set(["temp_desk_3", "note_full"]),
  );
  // 중소기업 deskSlot 4~5 는 간이 책상
  expect(props("small", [team("t", [boss()], { deskSlot: 5 })])).toEqual(new Set(["spare_desk_1"]));
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
  const root = layer.rootOf("a")!;
  expect(world.children).toContain(root);
  // 방이 바뀌어도 같은 컨테이너, 걸어서 옮긴다
  layer.sync(office([team("t", [boss(), emp("a", "Test")])]), "large", NOW);
  expect(layer.rootOf("a")).toBe(root);
  layer.update(NOW + WALK_MAX_MS);
  expect(root.position.x).toBe(screen(...(fp.stands.test[0] as [number, number])).x);
  // 외근은 출구까지 걸어가 숨는다
  layer.sync(office([team("t", [boss(), emp("a", "Outside")])]), "large", NOW + WALK_MAX_MS);
  expect(layer.rootOf("a")).toBe(root);
  layer.update(NOW + 2 * WALK_MAX_MS);
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

test("도착하면 걸어온 방향과 상관없이 뒤집지 않는다 (같은 스냅샷은 같은 모습)", () => {
  const { layer } = newLayer();
  // 테스트실(x≈350) → 서고(x≈212): 왼쪽으로 걸었어도 도착하면 그대로
  layer.sync(office([team("t", [boss(), emp("a", "Test")])]), "large", NOW);
  const root = layer.rootOf("a")!;
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

test("{elapsed} 는 Activity.elapsed 일 때만 채운다", () => {
  const { layer, overlay } = newLayer();
  const a = emp("a", "Test");
  a.activity.sentence = "{elapsed} 초 걸렸습니다";
  layer.sync(office([team("t", [boss(), a])]), "large", NOW);
  layer.hover("a");
  expect(overlay.querySelector(".ow-nametag span")!.textContent).toBe("{elapsed} 초 걸렸습니다");
});

test("스냅샷으로 다른 자리에 옮겨지면 이름표를 닫는다", () => {
  const { layer, overlay } = newLayer();
  layer.sync(office([team("t", [boss(), emp("a", "Test")])]), "large", NOW);
  layer.hover("a");
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
  expect(layer.rootOf("boss")!.children[0]).toBeInstanceOf(Graphics);
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
const posOf = (layer: ActorLayer, id: string) => {
  const r = layer.rootOf(id)!;
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
const bodyOf = (layer: ActorLayer, id: string) =>
  (layer.rootOf(id)!.children[0] as Sprite).texture.label;

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
  expect(layer.rootOf("a")!.visible).toBe(true);
  layer.update(NOW + WALK_MAX_MS);
  expect(layer.rootOf("a")!.visible).toBe(false);
  expect(layer.rootOf("b")!.visible).toBe(true);
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
  expect(layer.rootOf("a")!.scale.x).toBe(-1);
});

test("Rehire: 탕비실에 보이던 서브는 있던 자리에서 걷고, 숨어 있던 직원만 문에서 들어온다", () => {
  const { layer } = newLayer();
  const b = emp("b", "Pantry", "Sit");
  const c = emp("c", "Dev", "Type", { seat: 1 });
  layer.sync(office([team("t", [boss(), b, c])]), "large", NOW);
  const gone = { ...c, activity: { ...c.activity, room: "Outside" as Room } };
  layer.sync(office([team("t", [boss(), b, gone])]), "large", NOW);
  layer.update(NOW + WALK_MAX_MS);
  expect(layer.rootOf("c")!.visible).toBe(false);
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

test("두 sync 사이에 지나간 DTO 의 cue(earlier)도 재생한다", () => {
  const { layer } = newLayer();
  layer.sync(office([team("t", [boss()])]), "large", NOW);
  const o = office([team("t", [boss(), emp("a", "Dev", "Type", { seat: 0 })])]);
  layer.sync(o, "large", NOW, { earlier: [cue({ kind: "Enter", agent: "a" })] });
  expect(posOf(layer, "a")).toEqual(screen(...DOOR));
});
