import { castOf, owlFrameKeys, speciesOf } from "./office/actors.ts";
import { themeOf } from "./office/floorplan.ts";
import { loadOwls, type Animal } from "./office/sprites.ts";
import type { EmployeeDto } from "./types/ipc.ts";

type Employee = Pick<EmployeeDto, "agentId" | "rank" | "role">;
let animal: Animal = "mixed";
let atlas: ReturnType<typeof loadOwls> | undefined;
let pictures = new Map<string, HTMLCanvasElement>();
const requests = new WeakMap<HTMLElement, string>();
// 붙어 있는 아바타 자리. 동물이 바뀌면 다시 그리려고 둔다.
// 요소를 강하게 붙들면 제거된 카드·팝업이 회수되지 않으므로 WeakRef 로 두고 호출 때마다 죽은 자리를 지운다
type Mount = { ref: WeakRef<HTMLElement>; employee: Employee; building?: string };
const mounted = new Set<Mount>();
const mountOf = new WeakMap<HTMLElement, Mount>();

function prune(): void {
  for (const m of mounted) if (!m.ref.deref()?.isConnected) mounted.delete(m);
}

/** 테스트용 */
export function mountedCount(): number {
  return mounted.size;
}
let warned = false;

/** 동물이 바뀌면 그림 캐시를 버리고 붙어 있는 아바타를 모두 다시 그린다 (HANDOFF 4.6). 시트는 동물과 무관해 그대로 둔다 */
export function setAvatarAnimal(next: Animal): void {
  if (next === animal) return;
  animal = next;
  pictures = new Map();
  prune();
  for (const { ref, employee, building } of mounted) {
    const host = ref.deref();
    if (host) setAvatar(host, employee, building);
  }
}

async function picture(
  doc: Document,
  employee: Employee,
  building?: string,
): Promise<HTMLCanvasElement> {
  const { agentId, rank, role: duty } = employee;
  const cache = pictures; // await 중 동물이 바뀌어도 옛 그림이 새 캐시에 섞이지 않게
  const loading = (atlas ??= loadOwls(2));
  const owls = await loading.catch((error: unknown) => {
    // 일시 오류의 거부된 Promise 가 남아 영영 빈 자리표시가 되지 않게 한다
    if (atlas === loading) atlas = undefined;
    throw error;
  });
  const [w, h] = owls.meta.frame;
  if (w !== 32 || h !== 48) throw new Error("아바타 시트가 2× 32×48 이 아니다");
  const { employees } = castOf(owls.meta, animal);
  if (employees.length === 0) throw new Error("직원 종 목록이 비어 있다");
  const species = speciesOf(agentId, employees);
  const outfit = `${rank.toLowerCase()}-${themeOf(building)}`;
  const role = duty === "Trainee" ? null : duty.toLowerCase();
  const keys = owlFrameKeys(owls.textures, species, outfit, role, { pose: "idle", dir: "f" }, 0);
  if (!keys[0] || !keys[1]) throw new Error("아바타 몸체 또는 직급 프레임이 없다");
  const key = keys.join("|");
  const cached = cache.get(key);
  if (cached) return cached;
  const canvas = doc.createElement("canvas");
  canvas.width = 40;
  canvas.height = 44;
  const ctx = canvas.getContext("2d");
  if (!ctx) throw new Error("아바타 2d 컨텍스트를 만들 수 없다");
  for (const k of keys) {
    if (!k) continue;
    const texture = owls.textures[k];
    const f = texture.frame;
    if (f.width !== 32 || f.height !== 48) throw new Error("아바타 프레임 크기가 32×48이 아니다");
    ctx.drawImage(texture.source.resource as CanvasImageSource, f.x, f.y + 4, 32, 44, 4, 0, 32, 44);
  }
  cache.set(key, canvas);
  return canvas;
}

/** 늦게 온 아틀라스가 재사용한 카드나 닫힌 팝업에 옛 직원을 그리지 않게 한다. */
export function setAvatar(host: HTMLElement, employee: Employee, building?: string): void {
  const key = JSON.stringify([
    animal,
    employee.agentId,
    employee.rank,
    employee.role,
    themeOf(building),
  ]);
  if (requests.get(host) === key) return;
  requests.set(host, key);
  prune();
  const mount = mountOf.get(host);
  if (mount) Object.assign(mount, { employee, building });
  else {
    const added = { ref: new WeakRef(host), employee, building };
    mountOf.set(host, added);
    mounted.add(added);
  }
  host.replaceChildren();
  host.style.setProperty("--ow-avatar-bg", `var(--ow-${themeOf(building)}-tint)`);
  void picture(host.ownerDocument, employee, building)
    .then((source) => {
      if (requests.get(host) !== key || !host.isConnected) return;
      const canvas = host.ownerDocument.createElement("canvas");
      canvas.width = 40;
      canvas.height = 44;
      const ctx = canvas.getContext("2d");
      if (!ctx) throw new Error("아바타 2d 컨텍스트를 만들 수 없다");
      ctx.drawImage(source, 0, 0);
      host.replaceChildren(canvas);
    })
    .catch((error: unknown) => {
      // 다음 setAvatar 가 다시 시도하게 한다. 그 사이 키가 바뀌었으면 건드리지 않는다
      if (requests.get(host) === key) requests.delete(host);
      if (warned) return;
      warned = true;
      console.error("아바타를 그릴 수 없어 자리표시를 유지한다", error);
    });
}
