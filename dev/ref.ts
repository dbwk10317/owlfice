// 개발용 참조 비교 페이지 (npm run dev 에서 /dev/ref.html). 앱 빌드에 들어가지 않는다.
import { Rectangle } from "pixi.js";
import { CANVAS_H, CANVAS_W, type Kind, type Theme } from "../src/office/floorplan.ts";
import { buildScene, createOfficeApp } from "../src/office/renderer.ts";
import { loadOfficeAssets } from "../src/office/sprites.ts";

// tools/ref_render.py 의 CASES 와 같아야 한다
const CASES: [Kind, Theme][] = [
  ["large", "claude"],
  ["small", "codex"],
];
// 참조가 반투명인 픽셀은 채널당 이만큼 허용 (HANDOFF 6.7)
const TOLERANCE = 2;

declare global {
  interface Window {
    __refDiff?: Record<string, number>;
  }
}

async function loadRef(name: string): Promise<Uint8ClampedArray> {
  const img = new Image();
  img.src = `/dev/ref/${name}.png`;
  await img.decode();
  const c = document.createElement("canvas");
  c.width = CANVAS_W;
  c.height = CANVAS_H;
  const ctx = c.getContext("2d", { willReadFrequently: true })!;
  ctx.drawImage(img, 0, 0);
  return ctx.getImageData(0, 0, CANVAS_W, CANVAS_H).data;
}

/** 미리 곱한 알파를 나눠 곧은 알파로 돌려 화면에 보인다 (putImageData 는 곧은 알파를 받는다). */
function unpremultiply(data: Uint8ClampedArray): Uint8ClampedArray {
  const out = new Uint8ClampedArray(data.length);
  for (let i = 0; i < data.length; i += 4) {
    const a = data[i + 3];
    for (let k = 0; k < 3; k++) out[i + k] = a ? Math.round((data[i + k] * 255) / a) : 0;
    out[i + 3] = a;
  }
  return out;
}

function show(parent: HTMLElement, data: Uint8ClampedArray): void {
  const c = document.createElement("canvas");
  c.width = CANVAS_W;
  c.height = CANVAS_H;
  c.getContext("2d")!.putImageData(
    new ImageData(new Uint8ClampedArray(data), CANVAS_W, CANVAS_H),
    0,
    0,
  );
  parent.append(c);
}

/** 참조(곧은 알파)를 미리 곱해 Pixi 출력(미리 곱한 알파)과 견준다. 불투명 픽셀은 완전히 같아야 한다. */
function compare(
  ref: Uint8ClampedArray,
  got: Uint8ClampedArray,
): { count: number; diff: Uint8ClampedArray } {
  const diff = new Uint8ClampedArray(ref.length);
  let count = 0;
  for (let i = 0; i < ref.length; i += 4) {
    const a = ref[i + 3];
    // ponytail: 반투명 소품(김·장애 빛) 자리 ±2 는 그 소품이 참조에 들어오는 T23·T26 에서 마스크로
    const opaque = a === 255;
    let bad = false;
    for (let k = 0; k < 4; k++) {
      const want = k === 3 ? a : Math.round((ref[i + k] * a) / 255);
      if (Math.abs(want - got[i + k]) > (opaque ? 0 : TOLERANCE)) bad = true;
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
  for (const [kind, theme] of CASES) {
    const name = `${kind}-${theme}`;
    const scene = buildScene(assets, kind, theme, false);
    app.stage.addChild(scene.root);
    const { pixels } = app.renderer.extract.pixels({
      target: app.stage,
      frame: new Rectangle(0, 0, CANVAS_W, CANVAS_H),
      resolution: 1,
    });
    scene.root.destroy({ children: true });
    const ref = await loadRef(name);
    const { count, diff } = compare(ref, pixels);
    result[name] = count;
    const sec = document.createElement("section");
    const h = document.createElement("h2");
    h.textContent = `${name}: 다른 픽셀 ${count}`;
    if (count) h.className = "bad";
    sec.append(h);
    show(sec, ref);
    show(sec, unpremultiply(pixels));
    show(sec, diff);
    out.append(sec);
  }
  window.__refDiff = result;
}

main().catch((e) => {
  document.getElementById("out")!.textContent = `실패: ${e}`;
  console.error(e);
});
