// 테스트 전용: OfficeDto 조각을 짧게 만든다.
import type { Activity, EmployeeDto, OfficeDto, TeamDto } from "./types/ipc.ts";

export const iso = (h: number, m: number, s = 0): string =>
  new Date(2026, 0, 1, h, m, s).toISOString();

export function act(over: Partial<Activity> = {}): Activity {
  return {
    sentence: "열람 중",
    room: "Dev",
    since: iso(14, 0),
    expectedSecs: null,
    pose: "Type",
    confidence: "Confirmed",
    extra: 0,
    elapsed: false,
    ...over,
  };
}

export function emp(id: string, over: Partial<EmployeeDto> = {}): EmployeeDto {
  return {
    agentId: id,
    name: `이름${id}`,
    rank: "Daeri",
    role: "Backend",
    model: "opus-5.5",
    description: null,
    activity: act(),
    recent: [],
    counters: { read: 1, edit: 2, shell: 3, outside: 4, meeting: 5, errors: 6 },
    lastSay: null,
    hiredAt: iso(13, 0),
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
    ...over,
  };
}

export function team(id: string, employees: EmployeeDto[], over: Partial<TeamDto> = {}): TeamDto {
  return {
    teamId: id,
    title: `팀${id}`,
    building: "A",
    hostBadge: null,
    delayed: false,
    folder: `folder-${id}`,
    started: iso(13, 0),
    lastActivity: iso(14, 0),
    phase: "Working",
    workType: "NewDev",
    deskSlot: null,
    employees,
    props: {
      bossVisit: null,
      plant: null,
      light: "Off",
      lightAt: iso(13, 0),
      test: null,
      testAt: iso(13, 0),
      trash: 0,
      safeSince: iso(13, 0),
      pizza: 0,
      kanban: [],
      night: false,
      pr: null,
    },
    incident: null,
    idleAlive: false,
    ...over,
  };
}

export function office(
  officeId: string,
  stateSeq: number,
  teams: TeamDto[],
  cues: OfficeDto["cues"] = [],
): OfficeDto {
  return {
    stateSeq,
    officeId,
    rebuilt: false,
    teams,
    shared: {
      light: "Off",
      safeSince: iso(13, 0),
      trash: 0,
      pizza: 0,
      frameTitle: "",
      kanban: [],
      incident: false,
      night: false,
      test: null,
    },
    cues,
  };
}
