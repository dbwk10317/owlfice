// 사무실 시트를 읽고 시트 규약(HANDOFF 6.3)대로 스프라이트를 만든다.
import { Assets, Sprite, Spritesheet, Texture } from "pixi.js";
import type { OwlMeta, Owls } from "./actors.ts";
import { hexToRgb, parseFloorplan, type Floorplan } from "./floorplan.ts";

interface FrameData {
  frame: { x: number; y: number; w: number; h: number };
  anchorPx: [number, number];
}

export interface OfficeAssets {
  sheet: Spritesheet;
  floorplan: Floorplan;
}

// 배포 CSP(default-src 'self')가 Assets 의 blob: 워커와 data: 이미지 지원 감지를 막아 로드가 끝나지 않는다. PNG 만 쓰니 둘 다 끈다
let assetsReady: Promise<void> | null = null;
function initAssets(): Promise<void> {
  assetsReady ??= Assets.init({ skipDetections: true, preferences: { preferWorkers: false } });
  return assetsReady;
}

/** 시트·평면도는 Vite publicDir(design/sprites) 라 루트 경로로 읽는다 (HANDOFF 0). */
export async function loadOfficeAssets(): Promise<OfficeAssets> {
  await initAssets();
  const [sheet, raw] = await Promise.all([
    Assets.load<Spritesheet>("/office.json"),
    fetch("/floorplan.json").then((r) => {
      if (!r.ok) throw new Error(`floorplan.json ${r.status}`);
      return r.json();
    }),
  ]);
  return { sheet, floorplan: parseFloorplan(raw) };
}

export type { Animal } from "../types/ipc.ts";
export type Density = 2 | 3;

/** 캔버스 배율 1·2 는 2× 시트, 3 은 3× 시트 (design/HANDOFF.md 6.4). */
export function sheetDensity(scale: number): Density {
  return scale >= 3 ? 3 : 2;
}

/** 직원 시트. 동물과 상관없이 밀도마다 한 장이다. 실패는 부르는 쪽이 받아 대체 몸체로 그린다 (design/HANDOFF.md 11절). */
export async function loadOwls(density: Density = 2): Promise<Owls> {
  await initAssets();
  const path = `/employees${density === 3 ? "@3x" : ""}.json`;
  const sheet = await Assets.load<Spritesheet>(path);
  const meta = (sheet.data as { owlfice?: OwlMeta }).owlfice;
  if (!meta) throw new Error(`${path} 에 owlfice 메타 없음`);
  // 밀도를 잘못 읽으면 직원이 두 배·반 크기로 그려진다
  if (meta.density !== density) throw new Error(`${path} 밀도 ${meta.density}, 요청 ${density}`);
  return { textures: sheet.textures, meta };
}

function frameData(sheet: Spritesheet, key: string): FrameData {
  const f = (sheet.data.frames as Record<string, FrameData | undefined>)[key];
  if (!f) throw new Error(`office.json 에 프레임 ${key} 없음`);
  return f;
}

/** 기준점 픽셀의 왼쪽 위 모서리가 (x, y) 에 오는 스프라이트. 비율 anchor 는 소수 오차가 나서 anchorPx 를 pivot 으로. */
export function placeSprite(
  sheet: Spritesheet,
  key: string,
  x: number,
  y: number,
  texture?: Texture,
): Sprite {
  const [ax, ay] = frameData(sheet, key).anchorPx;
  const s = new Sprite(texture ?? sheet.textures[key]);
  // Spritesheet 가 JSON 의 비율 anchor 를 defaultAnchor 로 넣어 두므로 지우지 않으면 pivot 과 겹친다
  s.anchor.set(0);
  s.pivot.set(ax, ay);
  s.position.set(x, y);
  return s;
}

const tintCache = new Map<string, Texture>();

/**
 * 곱하기 색을 미리 구운 텍스처. 참조 렌더(PIL)는 채널마다 `v * t // 255` 로 내림하고 GPU tint 는 반올림이라
 * 픽셀 비교(HANDOFF 6.7)에서 1씩 어긋난다. 파티션처럼 자주 안 바뀌는 색만 이 길로.
 */
export function tintedTexture(sheet: Spritesheet, key: string, hex: string): Texture {
  const cacheKey = `${key}|${hex}`;
  const hit = tintCache.get(cacheKey);
  if (hit) return hit;
  const { x, y, w, h } = frameData(sheet, key).frame;
  const canvas = document.createElement("canvas");
  canvas.width = w;
  canvas.height = h;
  const ctx = canvas.getContext("2d", { willReadFrequently: true });
  if (!ctx) throw new Error("2d 컨텍스트를 만들 수 없다");
  ctx.drawImage(sheet.textureSource.resource as CanvasImageSource, x, y, w, h, 0, 0, w, h);
  const img = ctx.getImageData(0, 0, w, h);
  const [tr, tg, tb] = hexToRgb(hex);
  const d = img.data;
  for (let i = 0; i < d.length; i += 4) {
    d[i] = Math.floor((d[i] * tr) / 255);
    d[i + 1] = Math.floor((d[i + 1] * tg) / 255);
    d[i + 2] = Math.floor((d[i + 2] * tb) / 255);
  }
  ctx.putImageData(img, 0, 0);
  const tex = Texture.from(canvas);
  tintCache.set(cacheKey, tex);
  return tex;
}
