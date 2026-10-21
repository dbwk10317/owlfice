// 개발용 소품 상태 비교 페이지 (npm run dev 에서 /dev/props.html). 앱 빌드에 들어가지 않는다.
// 바뀐 소품 상태를 PropsLayer 로 그려 dev/props_ref.py 가 만든 참조와 견준다.
import { Rectangle } from "pixi.js";
import { CANVAS_H, CANVAS_W, hexToRgb, type Kind, type Theme } from "../src/office/floorplan.ts";
import { PropsLayer } from "../src/office/props.ts";
import { buildScene, createOfficeApp } from "../src/office/renderer.ts";
import { loadOfficeAssets } from "../src/office/sprites.ts";
import type { Light, OfficeDto, TeamDto, TodoItem } from "../src/types/ipc.ts";

declare global {
  interface Window {
    __propsDiff?: Record<string, number>;
  }
}

const todo = (status: TodoItem["status"], n: number): TodoItem[] =>
  Array.from({ length: n }, () => ({ content: "x", status }));
const team = (deskSlot: number, plant: number | null): TeamDto =>
  ({ teamId: `t${deskSlot}`, deskSlot, props: { plant } }) as unknown as TeamDto;
const office = (shared: Record<string, unknown>, teams: TeamDto[]): OfficeDto =>
  ({ teams, shared }) as unknown as OfficeDto;
const NOW = (h: number) => new Date(2026, 9, 21, h, 0).getTime();
const since = (now: number, min: number) => new Date(now - min * 60_000).toISOString();

interface Case {
  ref: string;
  kind: Kind;
  theme: Theme;
  lit: boolean;
  office: OfficeDto | null;
  now: number;
}

// dev/props_ref.py 의 CASES 와 같은 상태여야 한다
const CASES: Case[] = [
  {
    ref: "large-claude",
    kind: "large",
    theme: "claude",
    lit: false,
    office: null,
    now: NOW(12),
  },
  {
    ref: "props-large-claude",
    kind: "large",
    theme: "claude",
    lit: false,
    now: NOW(18),
    office: office(
      {
        light: "Green" as Light,
        safeSince: since(NOW(18), 37),
        trash: 13,
        pizza: 2,
        night: false,
        kanban: [...todo("Pending", 2), ...todo("InProgress", 1), ...todo("Done", 2)],
      },
      [team(0, 0.7)],
    ),
  },
  {
    ref: "props-small-codex-night",
    kind: "small",
    theme: "codex",
    lit: true,
    now: NOW(23),
    office: office(
      {
        light: "Red" as Light,
        safeSince: since(NOW(23), 1234),
        trash: 20,
        pizza: 8,
        night: true,
        kanban: [...todo("Pending", 1), ...todo("InProgress", 2), ...todo("Done", 1)],
      },
      [team(0, 0.5), team(1, 0.9), team(3, 1.5)],
    ),
  },
];

async function loadRef(name: string): Promise<Uint8ClampedArray> {
  const img = new Image();
  // decode() 는 캐시된 이미지에서 안 끝나는 때가 있어 load 이벤트를 쓴다
  await new Promise<void>((ok, fail) => {
    img.onload = () => ok();
    img.onerror = () => fail(new Error(`${name}.png 를 못 읽음`));
    img.src = `/dev/ref/${name}.png`;
  });
  const c = document.createElement("canvas");
  c.width = CANVAS_W;
  c.height = CANVAS_H;
  const ctx = c.getContext("2d", { willReadFrequently: true })!;
  ctx.drawImage(img, 0, 0);
  return ctx.getImageData(0, 0, CANVAS_W, CANVAS_H).data;
}

function show(parent: HTMLElement, data: Uint8ClampedArray, unpremultiply: boolean): void {
  const out = new Uint8ClampedArray(data);
  if (unpremultiply)
    for (let i = 0; i < out.length; i += 4) {
      const a = out[i + 3];
      for (let k = 0; k < 3; k++) out[i + k] = a ? Math.round((data[i + k] * 255) / a) : 0;
    }
  const c = document.createElement("canvas");
  c.width = CANVAS_W;
  c.height = CANVAS_H;
  c.getContext("2d")!.putImageData(new ImageData(out, CANVAS_W, CANVAS_H), 0, 0);
  parent.append(c);
}

/** 참조(곧은 알파)를 미리 곱해 Pixi 출력과 견준다. 조명을 곱한 장면은 채널당 1까지 허용 (HANDOFF 6.7) */
function compare(ref: Uint8ClampedArray, got: Uint8ClampedArray, tolerance: number) {
  const diff = new Uint8ClampedArray(ref.length);
  let count = 0;
  for (let i = 0; i < ref.length; i += 4) {
    const a = ref[i + 3];
    const tol = a === 255 ? tolerance : Math.max(2, tolerance);
    let bad = false;
    for (let k = 0; k < 4; k++) {
      const want = k === 3 ? a : Math.round((ref[i + k] * a) / 255);
      if (Math.abs(want - got[i + k]) > tol) bad = true;
    }
    if (bad) {
      count++;
      diff.set([255, 0, 0, 255], i);
    } else diff.set([ref[i], ref[i + 1], ref[i + 2], a / 4], i);
  }
  return { count, diff };
}

async function main(): Promise<void> {
  const out = document.getElementById("out")!;
  const app = await createOfficeApp();
  const assets = await loadOfficeAssets();
  const result: Record<string, number> = {};
  for (const c of CASES) {
    const scene = buildScene(assets, c.kind, c.theme, false);
    new PropsLayer(assets.floorplan, assets.sheet, c.kind, scene).update(c.office, c.now);
    if (c.lit) {
      const [r, g, b] = hexToRgb(assets.floorplan.lightTints.night);
      for (const l of scene.lit) l.tint = (r << 16) | (g << 8) | b;
    }
    app.stage.addChild(scene.root);
    const { pixels } = app.renderer.extract.pixels({
      target: app.stage,
      frame: new Rectangle(0, 0, CANVAS_W, CANVAS_H),
      resolution: 1,
    });
    scene.root.destroy({ children: true });
    const { count, diff } = compare(await loadRef(c.ref), pixels, c.lit ? 1 : 0);
    result[c.ref] = count;
    const sec = document.createElement("section");
    const h = document.createElement("h2");
    h.textContent = `${c.ref}: 다른 픽셀 ${count}`;
    if (count) h.className = "bad";
    sec.append(h);
    show(sec, await loadRef(c.ref), false);
    show(sec, pixels, true);
    show(sec, diff, false);
    out.append(sec);
  }
  window.__propsDiff = result;
}

main().catch((e) => {
  document.getElementById("out")!.textContent = `실패: ${e}`;
  console.error(e);
});
