// 사무실 캔버스: PixiJS 초기화, 정수 배율, 그리기 순서, 패닝·미니맵, 방 이름표 (HANDOFF 6.1, 6.2, 4.4, 2.2).
import { Application, Container, TextureStyle, UPDATE_PRIORITY } from "pixi.js";
import phrases from "../../design/handoff/phrases.ko.json";
import type { OfficeDto, OfficeListDto } from "../types/ipc.ts";
import {
  CANVAS_H,
  CANVAS_W,
  gridToScreen,
  hexToNumber,
  partitionTint,
  spriteName,
  themeOf,
  tileFrame,
  zIndexOf,
  type Kind,
  type LightName,
  type Theme,
} from "./floorplan.ts";
import { loadOfficeAssets, placeSprite, tintedTexture, type OfficeAssets } from "./sprites.ts";

const MINI_W = 120;
const MINI_H = 74;
const LIGHT_MS = 1500;
const RESIZE_DEBOUNCE_MS = 100;

/** 배율은 장치 픽셀 기준 정수 (PLAN 3.2). 백킹은 장치 픽셀, CSS 크기는 소수여도 된다. */
export function canvasScale(availW: number, availH: number, dpr: number) {
  const scale = Math.max(
    1,
    Math.floor(Math.min((availW * dpr) / CANVAS_W, (availH * dpr) / CANVAS_H)),
  );
  return {
    scale,
    backingW: CANVAS_W * scale,
    backingH: CANVAS_H * scale,
    cssW: (CANVAS_W * scale) / dpr,
    cssH: (CANVAS_H * scale) / dpr,
  };
}

/**
 * 창 크기 구간 (HANDOFF 2.2): 너비 720 미만은 가로로, 높이 480 미만은 세로로도 끌어서 이동.
 * 실제로 끌리는 것은 그 축에서 캔버스가 무대보다 클 때뿐이다.
 */
export function viewMode(winW: number, winH: number): { panX: boolean; panY: boolean } {
  return { panX: winW < 720, panY: winH < 480 };
}

/** 1× 캔버스 좌표 → 캔버스 왼쪽 위 기준 CSS px (HANDOFF 6.1). */
export function toCss(
  x: number,
  y: number,
  scale: number,
  dpr: number,
): { left: number; top: number } {
  return { left: (x * scale) / dpr, top: (y * scale) / dpr };
}

export async function createOfficeApp(): Promise<Application> {
  // 텍스처를 읽기 전에 정해야 시트가 nearest 로 올라간다
  TextureStyle.defaultOptions.scaleMode = "nearest";
  const app = new Application();
  await app.init({
    width: CANVAS_W,
    height: CANVAS_H,
    resolution: 1,
    antialias: false,
    roundPixels: true,
    backgroundAlpha: 0,
  });
  return app;
}

export interface Scene {
  root: Container;
  /** 조명 색을 곱하는 컨테이너. 창문과 장애 빛은 빠진다 (HANDOFF 6.2 7번) */
  lit: Container[];
}

/** render_scene 의 그리기 순서 그대로. 소품은 평면도의 기본 상태 (상태 반영은 T23). */
export function buildScene(
  assets: OfficeAssets,
  kind: Kind,
  theme: Theme,
  incident: boolean,
): Scene {
  const { sheet, floorplan: fp } = assets;
  const base = new Container();
  const win = new Container();
  const world = new Container();
  const over = new Container();
  world.sortableChildren = true;

  const o = gridToScreen(0, 0);
  base.addChild(placeSprite(sheet, `shell/${theme}/default`, o.x, o.y));
  for (let i = 0; i < fp.grid.cols; i++)
    for (let j = 0; j < fp.grid.rows; j++) {
      const p = gridToScreen(i, j);
      base.addChild(placeSprite(sheet, tileFrame(fp, i, j, theme, incident), p.x, p.y));
    }

  const placements = fp.layouts[kind].placements.filter((pl) => pl.visible !== false);
  // 깔개·벽 소품은 바닥 위에 배열 순서로, 월드는 depth 로 (render_scene 3·4단계)
  placements.forEach((pl, seq) => {
    const key = `${spriteName(pl, theme)}/${pl.state}`;
    const tint = pl.id.startsWith("dev_part_") ? partitionTint(fp, theme) : undefined;
    const s = placeSprite(
      sheet,
      key,
      pl.x,
      pl.y,
      tint ? tintedTexture(sheet, key, tint) : undefined,
    );
    if (pl.layer === "decal" || pl.layer === "wall") base.addChild(s);
    else if (pl.layer === "window") win.addChild(s);
    else if (pl.layer === "overlay") over.addChild(s);
    else {
      s.zIndex = zIndexOf(pl.depth, seq);
      world.addChild(s);
    }
  });

  const root = new Container();
  root.addChild(base, win, world, over);
  return { root, lit: [base, world] };
}

/** store 에서 렌더러가 읽는 부분만. */
interface OfficeStore {
  state: { list: OfficeListDto | null; selectedId: string | null; office: OfficeDto | null };
  subscribe(fn: () => void): () => void;
}

type Rgb = [number, number, number];
const WHITE: Rgb = [255, 255, 255];
const rgbOf = (hex: string): Rgb => {
  const n = hexToNumber(hex);
  return [(n >> 16) & 255, (n >> 8) & 255, n & 255];
};

/** `stage` 는 가운데 무대(`.ow-stage`). 캔버스·방 이름표·미니맵을 그 안에 만든다. */
export async function mountOffice(stage: HTMLElement, store: OfficeStore): Promise<void> {
  const app = await createOfficeApp();
  const assets = await loadOfficeAssets();
  const fp = assets.floorplan;

  const wrap = document.createElement("div");
  wrap.className = "ow-canvas-wrap";
  const overlay = document.createElement("div");
  overlay.className = "ow-overlay";
  wrap.append(app.canvas, overlay);
  const minimap = document.createElement("div");
  minimap.className = "ow-minimap";
  minimap.hidden = true;
  const miniInner = document.createElement("div");
  miniInner.style.position = "relative";
  const miniCanvas = document.createElement("canvas");
  miniCanvas.width = MINI_W;
  miniCanvas.height = MINI_H;
  miniCanvas.style.display = "block";
  const miniView = document.createElement("div");
  miniView.className = "ow-minimap__view";
  miniInner.append(miniCanvas, miniView);
  minimap.append(miniInner);
  stage.append(wrap, minimap);
  const miniCtx = miniCanvas.getContext("2d");

  let scene: Scene | null = null;
  let sceneKey = "";
  let labels: { el: HTMLElement; x: number; y: number }[] = [];
  let fit = canvasScale(CANVAS_W, CANVAS_H, 1);
  let dpr = 1;
  const pan = { x: 0, y: 0, maxX: 0, maxY: 0 };

  // ---------- 조명: 바뀌면 1.5초 보간 ----------
  let lightFrom: Rgb = WHITE;
  let lightTo: Rgb = WHITE;
  let lightCur: Rgb = WHITE;
  let lightT = 1;
  function setLight(name: LightName | null, immediate: boolean): void {
    const to = name ? rgbOf(fp.lightTints[name]) : WHITE;
    if (to.every((v, i) => v === lightTo[i])) return;
    lightFrom = immediate ? to : lightCur;
    lightTo = to;
    lightT = immediate ? 1 : 0;
  }
  function stepLight(deltaMs: number): void {
    if (!scene) return;
    lightT = Math.min(1, lightT + deltaMs / LIGHT_MS);
    lightCur = lightFrom.map((v, i) => Math.round(v + (lightTo[i] - v) * lightT)) as Rgb;
    const tint = (lightCur[0] << 16) | (lightCur[1] << 8) | lightCur[2];
    for (const c of scene.lit) c.tint = tint;
  }

  // ---------- 스냅샷 적용 ----------
  function apply(): void {
    const { list, office, selectedId } = store.state;
    if (!list) return;
    // 사무실을 바꾸는 중(선택은 있고 office 는 아직 null)에는 테마가 잠깐 바뀌어 깜빡이지 않게 지금 화면을 둔다
    if (!office && selectedId !== null && scene) return;
    const kind: Kind = list.scale === "Small" ? "small" : "large";
    const theme = themeOf(office?.teams[0]?.building);
    const incident = office?.shared.incident ?? false;
    const key = `${kind}|${theme}|${incident}`;
    const first = !scene;
    if (key !== sceneKey) {
      scene?.root.destroy({ children: true });
      scene = buildScene(assets, kind, theme, incident);
      app.stage.addChild(scene.root);
      sceneKey = key;
      buildLabels(kind);
    }
    // ponytail: dim(30분 조용 + 프로세스 살아 있음, S58)은 DTO 에 판정 필드가 없어 아직 안 켠다. 필드가 생기면 넣는다
    const off = !office || office.teams.every((t) => t.phase === "Off");
    setLight(off ? "off" : office.shared.night ? "night" : null, first);
    stepLight(0);
  }

  function buildLabels(kind: Kind): void {
    overlay.replaceChildren();
    const names = phrases.rooms as Record<string, string>;
    labels = Object.entries(fp.rooms).map(([k, room]) => {
      const el = document.createElement("div");
      el.className = "ow-roomlabel";
      el.textContent = (kind === "small" && k === "boss" ? names.bossSmall : names[k]) ?? room.name;
      overlay.append(el);
      return { el, x: room.label[0], y: room.label[1] };
    });
    placeLabels();
  }

  function placeLabels(): void {
    for (const l of labels) {
      const p = toCss(l.x, l.y, fit.scale, dpr);
      l.el.style.left = `${p.left}px`;
      l.el.style.top = `${p.top}px`;
    }
  }

  // ---------- 배율·패닝 ----------
  function layout(): void {
    dpr = window.devicePixelRatio || 1;
    const availW = stage.clientWidth;
    const availH = stage.clientHeight;
    fit = canvasScale(availW, availH, dpr);
    if (app.renderer.width !== fit.backingW || app.renderer.height !== fit.backingH)
      app.renderer.resize(fit.backingW, fit.backingH);
    app.stage.scale.set(fit.scale);
    for (const el of [wrap, app.canvas]) {
      el.style.width = `${fit.cssW}px`;
      el.style.height = `${fit.cssH}px`;
    }
    placeLabels();
    const mode = viewMode(window.innerWidth, window.innerHeight);
    // 무대는 가운데 정렬이라 넘친 만큼 양쪽으로 반씩 나간다
    pan.maxX = mode.panX ? Math.max(0, fit.cssW - availW) / 2 : 0;
    pan.maxY = mode.panY ? Math.max(0, fit.cssH - availH) / 2 : 0;
    const panning = pan.maxX > 0 || pan.maxY > 0;
    stage.classList.toggle("is-panning", panning);
    minimap.hidden = !panning;
    movePan(pan.x, pan.y);
  }

  function movePan(x: number, y: number): void {
    pan.x = Math.max(-pan.maxX, Math.min(pan.maxX, x));
    pan.y = Math.max(-pan.maxY, Math.min(pan.maxY, y));
    wrap.style.transform = pan.x || pan.y ? `translate(${pan.x}px, ${pan.y}px)` : "";
    if (minimap.hidden) return;
    // 보이는 영역 = 무대 ∩ 캔버스, 캔버스 좌표로
    const left = (stage.clientWidth - fit.cssW) / 2 + pan.x;
    const top = (stage.clientHeight - fit.cssH) / 2 + pan.y;
    const x0 = Math.max(0, -left);
    const y0 = Math.max(0, -top);
    const x1 = Math.min(fit.cssW, stage.clientWidth - left);
    const y1 = Math.min(fit.cssH, stage.clientHeight - top);
    const kx = MINI_W / fit.cssW;
    const ky = MINI_H / fit.cssH;
    miniView.style.left = `${x0 * kx}px`;
    miniView.style.top = `${y0 * ky}px`;
    miniView.style.width = `${(x1 - x0) * kx}px`;
    miniView.style.height = `${(y1 - y0) * ky}px`;
  }

  let drag: { id: number; sx: number; sy: number } | null = null;
  wrap.addEventListener("pointerdown", (e) => {
    if (minimap.hidden) return;
    drag = { id: e.pointerId, sx: e.clientX - pan.x, sy: e.clientY - pan.y };
    wrap.setPointerCapture(e.pointerId);
  });
  wrap.addEventListener("pointermove", (e) => {
    if (drag?.id === e.pointerId) movePan(e.clientX - drag.sx, e.clientY - drag.sy);
  });
  const endDrag = (e: PointerEvent) => {
    if (drag?.id === e.pointerId) drag = null;
  };
  wrap.addEventListener("pointerup", endDrag);
  wrap.addEventListener("pointercancel", endDrag);

  let timer: ReturnType<typeof setTimeout> | undefined;
  // device-pixel-content-box: CSS 크기는 그대로인데 DPR 만 바뀌어도(모니터 이동, 배율 변경) 다시 맞춘다
  new ResizeObserver(() => {
    clearTimeout(timer);
    timer = setTimeout(layout, RESIZE_DEBOUNCE_MS);
  }).observe(stage, { box: "device-pixel-content-box" });

  // ---------- 렌더 루프 ----------
  // 스냅샷은 표시만 해 두고 틱에서 한 번 적용한다. 숨은 창에선 틱이 멈춰 마지막 것만 보일 때 적용된다 (PLAN 4.9)
  let dirty = true;
  store.subscribe(() => {
    dirty = true;
  });
  app.ticker.maxFPS = 60;
  app.ticker.add(
    (t) => {
      if (dirty) {
        dirty = false;
        apply();
      }
      if (lightT < 1) stepLight(t.deltaMS);
    },
    undefined,
    UPDATE_PRIORITY.HIGH,
  );
  // 앱의 렌더(LOW) 뒤라 WebGL 버퍼가 아직 살아 있다 (preserveDrawingBuffer 없이 복사)
  app.ticker.add(
    () => {
      if (!minimap.hidden && miniCtx) miniCtx.drawImage(app.canvas, 0, 0, MINI_W, MINI_H);
    },
    undefined,
    UPDATE_PRIORITY.UTILITY,
  );
  document.addEventListener("visibilitychange", () => {
    if (document.hidden) app.ticker.stop();
    else app.ticker.start();
  });
  layout();
}
