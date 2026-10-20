// 사무실 시트를 읽고 시트 규약(HANDOFF 6.3)대로 스프라이트를 만든다.
import { Assets, Sprite, Spritesheet, Texture } from "pixi.js";
import { hexToRgb, parseFloorplan, type Floorplan } from "./floorplan.ts";

interface FrameData {
  frame: { x: number; y: number; w: number; h: number };
  anchorPx: [number, number];
}

export interface OfficeAssets {
  sheet: Spritesheet;
  floorplan: Floorplan;
}

/** 시트·평면도는 Vite publicDir(design/sprites) 라 루트 경로로 읽는다 (HANDOFF 0). */
export async function loadOfficeAssets(): Promise<OfficeAssets> {
  const [sheet, raw] = await Promise.all([
    Assets.load<Spritesheet>("/office.json"),
    fetch("/floorplan.json").then((r) => {
      if (!r.ok) throw new Error(`floorplan.json ${r.status}`);
      return r.json();
    }),
  ]);
  return { sheet, floorplan: parseFloorplan(raw) };
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
