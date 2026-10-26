import type { Daylight, EmployeeDto, OfficeDto, TeamDto, TodoItem } from "../types/ipc.ts";
import type { Kind, LightName } from "./floorplan.ts";

/** 부장 자세로 정해지는 책상 소품 (HANDOFF 5절): 응답 대기 김, 러버덕 집어 듦, 결재 대기 화면보호기 */
export interface BossFx {
  steam: boolean;
  duck: boolean;
  saver: boolean;
}

export function bossFx(team: TeamDto | undefined): BossFx {
  const a = team?.phase === "Off" ? undefined : team?.employees?.[0]?.activity;
  // 앉는 자세는 팀장실에 있을 때만 책상 앞이다. 서성임은 방과 상관없이 결재 대기다
  const seated = a?.room === "TeamLead";
  return {
    steam: seated && a.pose === "Think",
    duck: seated && a.pose === "Duck",
    saver: a?.pose === "Pace",
  };
}

/** 직원이 어느 방에 있나로 정해지는 빌드실·테스트실·출고장 소품 (HANDOFF 5절) */
export interface LabFx {
  /** 빌드실에 직원이 있다: 랙 LED 반복, 단말기 켜짐 */
  shell: boolean;
  /** 빌드실 직원이 쪼그려 앉았다 (명령이 평균 2배 초과): 선풍기 */
  slow: boolean;
  /** 테스트실에 직원이 있다: 테스트 기계 가동 */
  test: boolean;
}

// 빌드실·테스트실·출고장은 사무실이 하나뿐인 공용 방이라 어느 팀이든 일하면 켠다 (팀별로 가르는 bossFx 와 다른 이유)
// 백엔드 Room 하나가 셸 명령과 빌드를 같이 담아(DTO 로 가를 수 없다) 빌드 중에도 랙이 깜빡인다
export function labFx(office: OfficeDto | null): LabFx {
  const fx: LabFx = { shell: false, slow: false, test: false };
  for (const team of office?.teams ?? []) {
    if (team.phase === "Off") continue;
    for (const { activity: a } of team.employees) {
      if (a.room === "Build") {
        fx.shell = true;
        fx.slow ||= a.pose === "Squat";
      } else if (a.room === "Test") fx.test = true;
    }
  }
  return fx;
}

/** 공용 방 소품 (HANDOFF 5절 미팅·스킬·리뷰) */
export interface RoomFx {
  /** MCP 미팅 중인 거래처 상호. 없으면 null */
  vendor: string | null;
  /** 스킬 매뉴얼을 펼친 직원이 있다 */
  skill: boolean;
  /** 리뷰실에 직원이 있으면 빨간펜 수, 없으면 null */
  pens: number | null;
  /** 리뷰실 직원 중 승인 문구를 낸 사람이 있다 */
  approved: boolean;
}

// 미팅실·리뷰실도 공용 방이라 어느 팀이든 일하면 켠다 (labFx 와 같은 규칙)
export function roomFx(office: OfficeDto | null): RoomFx {
  const fx: RoomFx = { vendor: null, skill: false, pens: null, approved: false };
  for (const team of office?.teams ?? []) {
    if (team.phase === "Off") continue;
    for (const { meeting, review } of team.employees) {
      if (meeting?.kind === "Mcp") fx.vendor ??= meeting.server;
      else if (meeting) fx.skill = true;
      else if (review) {
        fx.pens = Math.max(fx.pens ?? 0, review.pens);
        fx.approved ||= review.approved;
      }
    }
  }
  return fx;
}

export type MagnetColor =
  | "red"
  | "blue"
  | "green"
  | "yellow"
  | "gray"
  | "teal"
  | "violet"
  | "coral";

// 대기업은 직원 순번, 중소기업은 팀장석(deskSlot) 번호로 색을 고른다. 팀 순서가 바뀌어도 색이 같다 (HANDOFF 6.6)
const MAGNET_BY_EMPLOYEE: MagnetColor[] = [
  "red",
  "blue",
  "green",
  "yellow",
  "gray",
  "teal",
  "violet",
  "coral",
];
const MAGNET_BY_TEAM: MagnetColor[] = ["teal", "violet", "coral", "yellow", "blue", "gray"];
/** 근태판 칸 번호 (props.attendance.columns) */
const COL_IN = 0;
const COL_OUT = 1;
const COL_NAP = 2;
const COL_OFF = 3;

export function attendanceColumn(team: TeamDto, emp: EmployeeDto): number {
  if (team.phase === "Off" || emp.departedAt !== null) return COL_OFF;
  if (emp.retiredAt !== null) return COL_NAP;
  if (emp.activity.room === "Outside") return COL_OUT;
  return emp.activity.room === "Pantry" && emp.activity.pose === "Sleep" ? COL_NAP : COL_IN;
}

export interface Magnet {
  col: number;
  line: number;
  color: MagnetColor;
}

/**
 * 직원마다 자석 하나: 퇴근한 팀·퇴근한 서브는 퇴근, 외근은 외근, 탕비실에서 조는 직원과 종료 뒤 탕비실의 서브는 외출, 나머지는 출근 (docs/CONTRACTS.md C3).
 * 한 칸에 슬롯 수까지만 (넘치면 뒤 직원은 자석이 없다). 중소기업은 모든 팀을 한 판에 합친다 (labFx 와 같은 규칙).
 */
export function magnets(office: OfficeDto | null, kind: Kind, slots: number): Magnet[] {
  const used = [0, 0, 0, 0];
  const out: Magnet[] = [];
  for (const team of office?.teams ?? [])
    team.employees.forEach((emp, i) => {
      const col = attendanceColumn(team, emp);
      const line = used[col]++;
      if (line >= slots) return;
      const color =
        kind === "large"
          ? MAGNET_BY_EMPLOYEE[i % MAGNET_BY_EMPLOYEE.length]
          : MAGNET_BY_TEAM[Math.min(team.deskSlot ?? MAGNET_BY_TEAM.length - 1, 5)];
      out.push({ col, line, color });
    });
  return out;
}

/**
 * 콘솔 게임기를 켜나. 탕비실에 있는 첫 직원이 콘솔 자리를 차지한다 (졸음은 소파, 서성임은 팀장실로 가서 뺀다).
 * actor-layout.ts layoutActors 가 콘솔에 앉히는 경우와 같다
 */
export function gameOn(office: OfficeDto | null, kind: Kind): boolean {
  return (office?.teams ?? []).some(
    (t) =>
      t.phase !== "Off" &&
      t.employees.some(
        (e, i) =>
          e.activity.room === "Pantry" &&
          e.activity.pose !== "Sleep" &&
          e.activity.pose !== "Pace" &&
          !(i === 0 && kind === "small" && t.deskSlot === null),
      ),
  );
}

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

export const windowState = (daylight: Daylight): string =>
  daylight === "evening" ? "dusk" : daylight;

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

/** 퇴근·대기 조명이 시간표보다 우선한다 (HANDOFF 6.2). */
export function lightOf(office: OfficeDto | null): LightName | null {
  const live = office?.teams.filter((t) => t.phase !== "Off") ?? [];
  if (live.length === 0) return "off";
  if (live.every((t) => t.idleAlive)) return "dim";
  const daylight = live[0].daylight;
  return daylight === "day" ? null : daylight;
}
