import { expect, test, vi } from "vite-plus/test";
import type { CueDto, OfficeDto, OfficeListDto, TeamDto } from "../types/ipc.ts";
import { team as testTeam, office as testOffice } from "../test-office.ts";
import {
  canvasAria,
  canvasScale,
  cueCollector,
  panLimits,
  maxFitScale,
  sheetSwitcher,
  teamOfProp,
  toCss,
} from "./renderer.ts";
import { lightOf } from "./prop-state.ts";
import type { Owls } from "./actor-appearance.ts";
import { sheetDensity, type Density } from "./sprites.ts";

test("액자 클릭은 포커스 팀을 따르고 팀장석 클릭은 자기 팀을 유지한다", () => {
  const older = testTeam("old", [], { deskSlot: 0, lastActivity: "2026-01-01T00:00:00Z" });
  const newer = testTeam("new", [], { deskSlot: 1, lastActivity: "2026-01-01T00:01:00Z" });
  const o = testOffice("vendor/Claude", 1, [older, newer]);
  expect(teamOfProp("rules_frame", o, "old")).toBe(older);
  expect(teamOfProp("boss_desk_1", o, "old")).toBe(newer);
  expect(teamOfProp("rules_frame", o)).toBe(newer);
  expect(teamOfProp("rules_frame", o, "gone")).toBe(newer);
});

test("배율은 장치 픽셀 기준 정수, CSS 크기는 백킹 / DPR", () => {
  // 2× 는 1040×640 장치 픽셀이 필요하다 (design/HANDOFF.md 6.1)
  expect(canvasScale(1040, 664, 1)).toEqual({
    scale: 2,
    backingW: 1040,
    backingH: 640,
    cssW: 1040,
    cssH: 640,
  });
  expect(canvasScale(1039, 664, 1).scale).toBe(1);
  expect(canvasScale(1560, 960, 1).scale).toBe(3);
  // 125%: 장치 1040 이면 2×, CSS 크기는 소수 없이 832
  expect(canvasScale(1040, 640, 1.25)).toEqual({
    scale: 2,
    backingW: 1040,
    backingH: 640,
    cssW: 832,
    cssH: 512,
  });
  // 경계: 실제 장치 픽셀이 1039 면 CSS 가 반올림돼 831.2→831 이어도 배율이 안 올라간다
  expect(canvasScale(1039, 640, 1.25).scale).toBe(1);
  // 150%: 1560×960 이면 3×, 그 밑이면 2×
  expect(canvasScale(1560, 960, 1.5)).toMatchObject({ scale: 3, cssW: 1040, cssH: 640 });
  expect(canvasScale(1559, 960, 1.5).scale).toBe(2);
  // 무대가 1× 보다 작아도 1 밑으로 내려가지 않는다 (패닝이 맡는다)
  expect(canvasScale(300, 200, 1)).toEqual({
    scale: 1,
    backingW: 520,
    backingH: 320,
    cssW: 520,
    cssH: 320,
  });
});

test("패닝은 축마다 캔버스가 무대보다 클 때만 열린다", () => {
  // 넘치지 않으면 어느 창 크기에서도 닫힘
  expect(panLimits(520, 320, 520, 320, 1)).toEqual({ maxX: 0, maxY: 0 });
  expect(panLimits(1040, 640, 1600, 900, 1)).toEqual({ maxX: 0, maxY: 0 });
  // 가로만 넘침: 반씩
  expect(panLimits(520, 320, 400, 400, 1)).toEqual({ maxX: 60, maxY: 0 });
  // 세로만 넘침, 큰 창이어도
  expect(panLimits(520, 320, 1000, 300, 1)).toEqual({ maxX: 0, maxY: 10 });
  // 장치 픽셀 차이는 CSS px 로
  expect(panLimits(520, 320, 400, 320, 2)).toEqual({ maxX: 30, maxY: 0 });
});

test("DOM 오버레이 좌표 = 1× 좌표 × scale / dpr", () => {
  expect(toCss(260, 180, 1, 1)).toEqual({ left: 260, top: 180 });
  expect(toCss(260, 180, 2, 1)).toEqual({ left: 520, top: 360 });
  expect(toCss(260, 180, 3, 1.5)).toEqual({ left: 520, top: 360 });
  expect(toCss(100, 50, 1, 1.25)).toEqual({ left: 80, top: 40 });
});

test("틱 사이에 온 DTO 들의 cue 를 모아 넘기고, sync 에 넘긴 DTO 의 cue 는 다시 넘기지 않는다", () => {
  const dto = (n: number) =>
    ({ cues: [{ teamId: "t", kind: "Enter", agent: `a${n}` }] as CueDto[] }) as OfficeDto;
  const [a, b, c, d] = [dto(1), dto(2), dto(3), dto(4)];
  const cues = cueCollector();
  cues.see(a, false);
  cues.see(b, false);
  cues.see(b, false);
  cues.see(c, false);
  // c 는 sync 가 직접 본다
  expect(cues.take(c)).toEqual([...a.cues, ...b.cues]);
  expect(cues.take(c)).toEqual([]);
  cues.see(d, false);
  expect(cues.take(d)).toEqual([]);
});

test("숨은 동안 see 가 계속 불려도 쌓이지 않고, 돌아온 뒤 숨기 전 DTO 의 cue 가 끼어들지 않는다", () => {
  const dto = (n: number) =>
    ({ cues: [{ teamId: "t", kind: "Enter", agent: `a${n}` }] as CueDto[] }) as OfficeDto;
  const [p, x, z] = [dto(1), dto(2), dto(3)];
  const hiddenDtos = [dto(11), dto(12), dto(13)];
  const cues = cueCollector();
  cues.see(p, false);
  for (const q of hiddenDtos) cues.see(q, true);
  // 돌아온 첫 sync 는 cue 를 버리니 take 는 비어야 한다
  expect(cues.take(hiddenDtos[2])).toEqual([]);
  cues.see(x, false);
  cues.see(z, false);
  expect(cues.take(z)).toEqual(x.cues);
});

test("사규 액자는 가장 최근 활동 팀, 팀장석 책상은 그 자리(deskSlot)의 팀을 가리킨다", () => {
  const t = (teamId: string, deskSlot: number, lastActivity: string) =>
    ({ teamId, deskSlot, lastActivity }) as TeamDto;
  const o = {
    teams: [
      t("a", 0, "2026-01-01T14:00:00Z"),
      t("b", 1, "2026-01-01T14:05:00Z"),
      t("c", 3, "2026-01-01T14:02:00Z"),
    ],
  } as OfficeDto;
  expect(teamOfProp("rules_frame", o)?.teamId).toBe("b");
  // 동점이면 뒤 팀 (백엔드 max_by_key)
  const tie = { teams: [t("x", 0, "2026-01-01T14:00:00Z"), t("y", 1, "2026-01-01T14:00:00Z")] };
  expect(teamOfProp("rules_frame", tie as OfficeDto)?.teamId).toBe("y");
  expect(teamOfProp("boss_desk_0", o)?.teamId).toBe("a");
  expect(teamOfProp("boss_desk_3", o)?.teamId).toBe("c");
  expect(teamOfProp("boss_desk_2", o)).toBeUndefined(); // 빈 책상
  expect(teamOfProp("rules_frame", null)).toBeUndefined();
  expect(teamOfProp("tv", o)).toBeUndefined();
});

test("캔버스 aria-label 은 그린 사무실의 이름·직원 수·장애를 따르고, 사무실이 없으면 없다", () => {
  const list = { offices: [{ officeId: "o1", label: "B동 203호" }] } as OfficeListDto;
  const emp = (n: number) => ({ employees: Array.from({ length: n }) }) as unknown as TeamDto;
  const office = (incident: boolean) =>
    ({ officeId: "o1", teams: [emp(2), emp(1)], shared: { incident } }) as OfficeDto;
  expect(canvasAria(office(false), list)).toBe("B동 203호 사무실 · 직원 3명");
  expect(canvasAria(office(true), list)).toBe("B동 203호 사무실 · 직원 3명 · 장애 중");
  expect(canvasAria(null, list)).toBeNull();
  expect(canvasAria({ ...office(false), officeId: "x" }, list)).toBeNull();
});

test("조명은 off > dim > DTO 시간대 순이며 공유 night 값과 독립이다", () => {
  const t = (phase: TeamDto["phase"], daylight: TeamDto["daylight"], idleAlive = false) =>
    ({ phase, daylight, idleAlive }) as TeamDto;
  const o = (...teams: TeamDto[]) => ({ teams, shared: { night: false } }) as OfficeDto;
  expect(lightOf(null)).toBe("off");
  for (const daylight of ["day", "evening", "night"] as const) {
    expect(lightOf(o(t("Off", daylight)))).toBe("off");
    expect(lightOf(o(t("Dozing", daylight, true), t("Off", daylight)))).toBe("dim");
    expect(lightOf(o(t("Dozing", daylight, true), t("Working", daylight)))).toBe(
      daylight === "day" ? null : daylight,
    );
  }
});

test("직원 시트 밀도: 배율 1·2 는 2×, 3 은 3×", () => {
  expect([1, 2, 3].map(sheetDensity)).toEqual([2, 2, 3]);
});

test("직원 시트 교체는 겹친 요청 중 마지막 것만 적용하고, 실패하면 지금 시트를 두고 다시 요청하면 또 읽는다", async () => {
  const pending = new Map<Density, { ok: (o: Owls) => void; fail: (e: Error) => void }>();
  const load = vi.fn(
    (density: Density) => new Promise<Owls>((ok, fail) => pending.set(density, { ok, fail })),
  );
  const sheet = (key: string) => ({ key }) as unknown as Owls;
  const applied: unknown[] = [];
  const pick = sheetSwitcher(2, load, (o) => applied.push((o as unknown as { key: string }).key));
  const settle = () => new Promise((r) => setTimeout(r));
  // 지금 그리는 시트는 다시 읽지 않는다
  pick(2);
  expect(load).not.toHaveBeenCalled();
  // 읽는 중에 지금 시트로 돌아오면 끝난 로드를 버린다
  pick(3);
  pick(2);
  pending.get(3)!.ok(sheet("3a"));
  await settle();
  expect(applied).toEqual([]);
  // 다시 3 으로 가면 새로 읽는다
  pick(3);
  expect(load).toHaveBeenCalledTimes(2);
  pending.get(3)!.ok(sheet("3b"));
  await settle();
  expect(applied).toEqual(["3b"]);
  // 실패: 알리고 지금 시트 유지, 같은 요청이 다시 오면 또 읽는다
  const error = vi.spyOn(console, "error").mockImplementation(() => {});
  pending.clear();
  pick(2);
  pending.get(2)!.fail(new Error("404"));
  await settle();
  expect(applied).toEqual(["3b"]);
  expect(error).toHaveBeenCalledTimes(1);
  error.mockRestore();
  pending.clear();
  pick(2);
  pending.get(2)!.ok(sheet("2"));
  await settle();
  expect(applied).toEqual(["3b", "2"]);
});

test("창에 맞는 배율은 3 을 넘지 않고 720 미만 창은 1 이다", () => {
  expect(maxFitScale(520 * 5, 320 * 5, 1, 3000)).toBe(3);
  expect(maxFitScale(520 * 2, 320 * 2, 1, 1300)).toBe(2);
  expect(maxFitScale(520 * 2, 320 * 2, 1, 700)).toBe(1);
});
