import type { Texture } from "pixi.js";
import type { Animal } from "../types/ipc.ts";
import type { Look } from "./actor-layout.ts";

/** 좌표는 모두 시트 텍셀 단위. `density` 로 나눠 월드 px 로 쓴다 (HANDOFF 6.3). */
export interface OwlMeta {
  density: number;
  frame: [number, number];
  anchor: [number, number];
  /** 섞기에서 팀마다 고르는 사장님 종 (동물마다 하나) */
  bossSpecies: string[];
  /** 섞기에서 고르는 직원 종 (세 동물의 직원 종을 이은 것) */
  employeeSpecies: string[];
  animals: Record<Exclude<Animal, "mixed">, { name: string; employees: string[]; boss: string }>;
  poses: Record<string, { dirs: Record<string, number>; fps: number }>;
  anchors: Record<string, { hand: [number, number]; head: [number, number] }>;
}

/** 직원 시트의 텍스처와 `owlfice` 메타. 로드에 실패하면 null 로 받아 대체 몸체를 그린다. */
export interface Owls {
  textures: Record<string, Texture>;
  meta: OwlMeta;
}

// ---------- 종 ----------

/** 백엔드 names.rs 의 fnv1a64 와 같다 (UTF-8 바이트). 종을 백엔드 이름과 같은 해시로 골라야 한다 (HANDOFF 6.4). */
export function fnv1a64(s: string): bigint {
  let h = 0xcbf29ce484222325n;
  for (const b of new TextEncoder().encode(s))
    h = ((h ^ BigInt(b)) * 0x100000001b3n) & 0xffffffffffffffffn;
  return h;
}

export function speciesOf(agentId: string, species: string[]): string {
  return species[Number(fnv1a64(agentId) % BigInt(species.length))];
}

/**
 * 설정의 직원 동물로 고를 직원 종 목록과 팀의 사장님 종 (HANDOFF 6.4 speciesRule).
 * 캔버스와 카드 아바타가 같이 써야 같은 직원이 같은 종으로 보인다.
 */
export function castOf(
  meta: OwlMeta,
  animal: Animal,
): { employees: string[]; bossOf(teamId: string): string } {
  if (animal === "mixed")
    return {
      employees: meta.employeeSpecies,
      bossOf: (teamId) => speciesOf(teamId, meta.bossSpecies),
    };
  const { employees, boss } = meta.animals[animal];
  return { employees, bossOf: () => boss };
}

// ---------- 직원 프레임 ----------

/**
 * 세 겹 프레임 이름 (HANDOFF 6.4). 시트에 없는 겹은 null: 수습은 담당 겹이 없고,
 * 담당 소품이 안 보이는 자세(예: 색상표의 type)는 담당 프레임이 없다.
 */
export function owlFrameKeys(
  textures: Record<string, Texture>,
  species: string,
  outfit: string,
  role: string | null,
  look: Look,
  frame: number,
): (string | null)[] {
  const name = `${look.pose}_${look.dir}_${frame}`;
  const keys = [
    `body/${species}/${name}`,
    `outfit/${outfit}/${name}`,
    role && `role/${role}/${name}`,
  ];
  return keys.map((k) => (k && textures[k] ? k : null));
}
