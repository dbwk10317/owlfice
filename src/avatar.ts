import { owlFrameKeys, speciesOf } from "./office/actors.ts";
import { themeOf } from "./office/floorplan.ts";
import { loadOwls } from "./office/sprites.ts";
import type { EmployeeDto } from "./types/ipc.ts";

type Employee = Pick<EmployeeDto, "agentId" | "rank" | "role">;
let atlas: ReturnType<typeof loadOwls> | undefined;
const pictures = new Map<string, HTMLCanvasElement>();
const requests = new WeakMap<HTMLElement, string>();
let warned = false;

async function picture(
  doc: Document,
  employee: Employee,
  building?: string,
): Promise<HTMLCanvasElement> {
  const { agentId, rank, role: duty } = employee;
  const owls = await (atlas ??= loadOwls());
  if (owls.meta.employeeSpecies.length === 0) throw new Error("올빼미 종 목록이 비어 있다");
  const species = speciesOf(agentId, owls.meta.employeeSpecies);
  const outfit = `${rank.toLowerCase()}-${themeOf(building)}`;
  const role = duty === "Trainee" ? null : duty.toLowerCase();
  const keys = owlFrameKeys(owls.textures, species, outfit, role, { pose: "idle", dir: "f" }, 0);
  if (!keys[0] || !keys[1]) throw new Error("아바타 몸체 또는 직급 프레임이 없다");
  const key = keys.join("|");
  const cached = pictures.get(key);
  if (cached) return cached;
  const canvas = doc.createElement("canvas");
  canvas.width = 20;
  canvas.height = 22;
  const ctx = canvas.getContext("2d");
  if (!ctx) throw new Error("아바타 2d 컨텍스트를 만들 수 없다");
  for (const k of keys) {
    if (!k) continue;
    const texture = owls.textures[k];
    const f = texture.frame;
    if (f.width !== 16 || f.height !== 24) throw new Error("아바타 프레임 크기가 16×24가 아니다");
    ctx.drawImage(texture.source.resource as CanvasImageSource, f.x, f.y + 2, 16, 22, 2, 0, 16, 22);
  }
  pictures.set(key, canvas);
  return canvas;
}

/** 늦게 온 아틀라스가 재사용한 카드나 닫힌 팝업에 옛 직원을 그리지 않게 한다. */
export function setAvatar(host: HTMLElement, employee: Employee, building?: string): void {
  const key = JSON.stringify([employee.agentId, employee.rank, employee.role, themeOf(building)]);
  if (requests.get(host) === key) return;
  requests.set(host, key);
  host.replaceChildren();
  host.style.setProperty("--ow-avatar-bg", `var(--ow-${themeOf(building)}-tint)`);
  void picture(host.ownerDocument, employee, building)
    .then((source) => {
      if (requests.get(host) !== key || !host.isConnected) return;
      const canvas = host.ownerDocument.createElement("canvas");
      canvas.width = 20;
      canvas.height = 22;
      const ctx = canvas.getContext("2d");
      if (!ctx) throw new Error("아바타 2d 컨텍스트를 만들 수 없다");
      ctx.drawImage(source, 0, 0);
      host.replaceChildren(canvas);
    })
    .catch((error: unknown) => {
      if (warned) return;
      warned = true;
      console.error("아바타를 그릴 수 없어 자리표시를 유지한다", error);
    });
}
