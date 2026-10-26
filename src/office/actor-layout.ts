import type { EmployeeDto, OfficeDto, Pose, Room, TeamDto } from "../types/ipc.ts";
import { empKey } from "../employee-key.ts";
import { gridToScreen, type Floorplan, type Kind } from "./floorplan.ts";

// ---------- 자리와 자세 (HANDOFF 5절) ----------

export interface Look {
  pose: string;
  dir: "f" | "b";
  held?: string;
}

interface Rule {
  /** "seat" 는 자기 좌석, 아니면 floorplan.stands 이름을 이은 목록 */
  at: "seat" | string[];
  look: Look;
  /** 서 있는 자리 이름별로 자세가 다를 때 (탕비실: 콘솔 sit, 안마의자 sleep, 그 밖 idle) */
  looks?: Record<string, Look>;
  /** 목록에서 먼저 볼 자리. 장애 상황실 0번은 부장 몫이다 */
  from?: number;
}

export const IDLE: Look = { pose: "idle", dir: "f" };
const SIT: Look = { pose: "sit", dir: "f" };

const SEAT_LOOK: Record<Exclude<Pose, "Pace">, Look> = {
  Sit: SIT,
  Type: { pose: "type", dir: "f" },
  Read: { pose: "read", dir: "f" },
  Think: { pose: "think", dir: "f" },
  Duck: { pose: "carry", dir: "f", held: "duck" },
  Squat: { pose: "crouch", dir: "f" },
  Sleep: { pose: "sleep", dir: "f" },
};

// 백엔드 Room 하나에 HANDOFF 5절 행이 여럿이면(셸·빌드, git 조회·큰 열람) DTO 로 가를 수 없어 자리 목록을 잇는다
const ROOM_RULE: Partial<Record<Room, Rule>> = {
  Archive: { at: ["library"], look: { pose: "carry", dir: "f", held: "folder" } },
  Build: { at: ["build_terminal", "build"], look: IDLE },
  Test: { at: ["test"], look: IDLE },
  Review: { at: ["review_sofa"], look: SIT },
  Meeting: { at: ["meeting"], look: IDLE },
  Conference: { at: ["conference"], look: { pose: "read", dir: "f" } },
  Shipping: { at: ["shipping"], look: { pose: "carry", dir: "f", held: "box_carry" } },
  Door: { at: ["door_wait", "door"], look: { pose: "carry", dir: "f", held: "clipboard" } },
  // 서브 종료 (HANDOFF 5절): 첫째 콘솔, 둘째 안마의자, 그다음은 탕비실에 서 있는다
  Pantry: {
    at: ["pantry_console", "pantry_massage", "pantry"],
    look: SIT,
    looks: { pantry_console: SIT, pantry_massage: SEAT_LOOK.Sleep, pantry: IDLE },
  },
  // 백엔드는 팀원을 장애 상황실로 보내지 않지만 표대로 둔다
  Incident: { at: ["incident"], look: IDLE, from: 1 },
  // 부장 아닌 직원이 팀장실, 부장이 개발실에 있을 때
  TeamLead: { at: ["boss_pace"], look: IDLE },
  Dev: { at: ["dev_center"], look: IDLE },
};

/** 방·자세 → 자리와 sprite 자세. null 은 그리지 않는다 (외근). */
function placeRule(room: Room, pose: Pose, boss: boolean): Rule | null {
  if (room === "Outside") return null;
  if (pose === "Pace") return { at: ["boss_pace"], look: IDLE };
  if (room === (boss ? "TeamLead" : "Dev")) return { at: "seat", look: SEAT_LOOK[pose] };
  if (room === "Incident" && boss) return { at: ["incident"], look: { pose: "panic", dir: "f" } };
  if (room === "Pantry" && pose === "Sleep")
    return { at: ["pantry_sofa", "pantry_massage"], look: SEAT_LOOK.Sleep };
  const rule = ROOM_RULE[room];
  if (!rule) return null;
  if (pose === "Squat" || pose === "Sleep")
    return { ...rule, looks: undefined, look: SEAT_LOOK[pose] };
  return rule;
}

// 앉는 자리는 가구와 같은 depth 에 두고 순번으로 가구 뒤에 그린다. gx + gy 로는 방석·소파 앞뒤가 뒤집힌다
const SIT_ON: Record<string, string> = {
  review_sofa: "review_seat",
  pantry_sofa: "sofa",
  pantry_massage: "massage_chair",
  roof: "roof_bench",
};

interface Spot {
  id: string;
  gx: number;
  gy: number;
  z: number;
  depth: number;
  desk?: string;
  look?: Look;
}

export interface Placed {
  emp: EmployeeDto;
  team: TeamDto;
  spot: string;
  x: number;
  y: number;
  gx: number;
  gy: number;
  z: number;
  depth: number;
  look: Look;
  seated: boolean;
}

/** 참조 렌더(Python round)와 같은 은행가 반올림. 의자·소파 자리 몇 곳이 정확히 .5 에 떨어진다. */
export function roundEven(v: number): number {
  const f = Math.floor(v);
  const d = v - f;
  if (d !== 0.5) return Math.round(v);
  return f % 2 === 0 ? f : f + 1;
}

function standSpots(fp: Floorplan, kind: Kind, rule: Rule & { at: string[] }): Spot[] {
  const placements = fp.layouts[kind].placements;
  return rule.at.flatMap((name) =>
    (fp.stands[name] ?? []).map(([gx, gy, z = 0], i) => {
      const on =
        SIT_ON[name] && (name !== "roof" || z > 0) && placements.find((p) => p.id === SIT_ON[name]);
      return {
        id: `${name}_${i}`,
        gx,
        gy,
        z,
        depth: on ? on.depth : gx + gy,
        look: rule.looks?.[name],
      };
    }),
  );
}

export function seatSpots(
  fp: Floorplan,
  kind: Kind,
  team: TeamDto,
  emp: EmployeeDto,
  boss: boolean,
): { spots: Spot[]; index: number } {
  const s = fp.layouts[kind].seats;
  const toSpot = (seat: {
    id: string;
    gx: number;
    gy: number;
    depth: number;
    desk: string;
  }): Spot => ({
    ...seat,
    z: 0,
  });
  if (!boss) {
    const spots = [...s.dev, ...s.temp].map(toSpot);
    // 백엔드는 10명째 뒤 직원에게 좌석을 안 준다(카드만). 마지막 임시 책상부터 보고, 차 있으면 layoutActors 가 "+n" 으로 센다
    return { spots, index: emp.seat ?? spots.length };
  }
  if (kind === "large") return { spots: s.boss.map(toSpot), index: 0 };
  // 팀장석 0~3, 간이 4~5 (design/HANDOFF.md 6.5). deskSlot null 은 layoutActors 가 먼저 거른다
  return { spots: [...s.boss, ...s.spare].map(toSpot), index: team.deskSlot ?? 0 };
}

/**
 * 스냅샷 → 그릴 직원과 자리. 순서는 팀 순서 → 직원 순서 (같은 depth 에서 그리는 순서, HANDOFF 6.2).
 * 같은 자리를 먼저 차지한 직원이 있으면 그 목록의 다음 자리, 다 찼으면 마지막 자리에 겹친다.
 * 서브 좌석과 탕비실 쉬는 자리는 겹치지 않는다: 남는 직원은 그리지 않고 `crowd` 에 마지막 자리 id → 남은 수로 센다 (HANDOFF 4.4, 6.5).
 */
export function layoutActors(
  fp: Floorplan,
  kind: Kind,
  office: OfficeDto,
  crowd = new Map<string, number>(),
  visits = new Map<string, "archive" | "security">(),
): Placed[] {
  const used = new Set<string>();
  const out: Placed[] = [];
  for (const team of office.teams) {
    // 퇴근한 팀은 모두 퇴장한다 (HANDOFF 5절)
    if (team.phase === "Off") continue;
    team.employees.forEach((emp, i) => {
      const boss = i === 0;
      // 팀장석·간이 책상이 다 찬 팀은 카드만 (design/HANDOFF.md 6.4·6.5). 상황실·탕비실에서도 자리를 차지하지 않는다
      if (boss && kind === "small" && team.deskSlot === null) return;
      // 퇴근한 서브는 캔버스에서 빠진다. 보이던 액터는 sync 가 출구까지 걷게 한 뒤 숨긴다 (HANDOFF 5절)
      if (emp.departedAt !== null) return;
      const visit = visits.get(empKey(team.teamId, emp.agentId));
      const rule = visit
        ? {
            at: [visit],
            look: {
              pose: "carry",
              dir: "f",
              held: visit === "security" ? "clipboard" : "folder",
            } as Look,
          }
        : team.idleAlive && emp.activity.room === "Pantry" && emp.activity.pose === "Sleep"
          ? {
              at: ["pantry_sofa", "pantry_massage", "pantry_console", "pantry", "roof"],
              look: SEAT_LOOK.Sleep,
            }
          : placeRule(emp.activity.room, emp.activity.pose, boss);
      if (!rule) return;
      let spots: Spot[];
      let start: number;
      if (rule.at === "seat") {
        const seat = seatSpots(fp, kind, team, emp, boss);
        spots = seat.spots;
        start = seat.index;
      } else {
        spots = standSpots(fp, kind, { ...rule, at: rule.at });
        start = rule.from ?? 0;
      }
      if (spots.length === 0) return;
      start = Math.min(Math.max(0, start), spots.length - 1);
      let k = start;
      while (k < spots.length && used.has(spots[k].id)) k++;
      if (
        k === spots.length &&
        ((rule.at === "seat" && !boss) ||
          emp.activity.room === "Pantry" ||
          emp.activity.room === "Conference" ||
          !!visit)
      ) {
        const last = spots[spots.length - 1].id;
        crowd.set(last, (crowd.get(last) ?? 0) + 1);
        return;
      }
      // ponytail: 그 밖의 방은 자리가 모자라면 마지막 자리에 겹친다, 겹침 연출은 T29
      const spot = spots[Math.min(k, spots.length - 1)];
      used.add(spot.id);
      // 스킬은 앉아서 매뉴얼을 펼치고 MCP 는 서서 미팅한다 (HANDOFF 5절)
      const skill = emp.activity.pose === "Sit" && emp.meeting?.kind === "Skill";
      const p = gridToScreen(spot.gx, spot.gy, spot.z);
      out.push({
        emp,
        team,
        spot: spot.id,
        x: roundEven(p.x),
        y: roundEven(p.y),
        gx: spot.gx,
        gy: spot.gy,
        z: spot.z,
        depth: spot.depth,
        look: spot.look ?? (skill ? SIT : rule.look),
        seated: rule.at === "seat",
      });
    });
  }
  return out;
}

/**
 * 좌석을 가진 직원과 그 좌석. 퇴근한 팀, 팀장석이 없는 중소기업 부장, 퇴장한 서브는 빠진다.
 * 좌석 배정(seat, deskSlot)으로 정하니 직원이 다른 방에 있어도 나온다.
 */
export function* seated(fp: Floorplan, kind: Kind, office: OfficeDto | null) {
  for (const team of office?.teams ?? []) {
    // 퇴근한 팀은 모두 퇴장했고 백엔드도 자리를 비운다
    if (team.phase === "Off") continue;
    for (const [i, emp] of team.employees.entries()) {
      const boss = i === 0;
      if (boss && kind === "small" && team.deskSlot === null) continue;
      // 퇴장한 서브는 백엔드가 자리를 비운다
      if (!boss && emp.retiredAt !== null) continue;
      const { spots, index } = seatSpots(fp, kind, team, emp, boss);
      yield { emp, spots, index, spot: spots[Math.min(index, spots.length - 1)] };
    }
  }
}

/**
 * 평면도가 숨겨 둔 소품 중 지금 보여야 할 것: 임시·간이 책상과 쪽지 (HANDOFF 6.5).
 * 임시 자리 직원이 다른 방에 가도 책상은 남는다.
 * 만석 쪽지는 임시 책상 쪽지와 같은 자리에 놓고 그것을 대신한다.
 */
export function seatProps(fp: Floorplan, kind: Kind, office: OfficeDto | null): Set<string> {
  const s = fp.layouts[kind].seats;
  const out = new Set<string>();
  let temp = false;
  let full = false;
  for (const { spots, index, spot } of seated(fp, kind, office)) {
    if (!spot?.desk) continue;
    // 팀장석 책상은 평면도에 늘 있다. 켜는 건 간이·임시 책상뿐
    const isTemp = s.temp.some((x) => x.id === spot.id);
    if (!isTemp && !s.spare.some((x) => x.id === spot.id)) continue;
    out.add(spot.desk);
    if (isTemp) {
      temp = true;
      full ||= index >= spots.length;
    }
  }
  if (temp) {
    // 접이식 책상은 7명째부터 4자리가 한꺼번에 펴진다 (design/HANDOFF.md 6.4·6.5)
    for (const x of s.temp) if (x.desk) out.add(x.desk);
    out.add(full ? "note_full" : "note_temp");
  }
  return out;
}
