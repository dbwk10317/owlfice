// 사무실 캔버스: PixiJS 초기화, 정수 배율, 그리기 순서, 패닝·미니맵, 방 이름표 (HANDOFF 6.1, 6.2, 4.4, 2.2).
import { Application, Container, TextureStyle, UPDATE_PRIORITY } from "pixi.js";
import phrases from "../../design/handoff/phrases.ko.json";
import { ActorLayer } from "./actors.ts";
import type { OfficeDto, OfficeListDto } from "../types/ipc.ts";
import {
  CANVAS_H,
  CANVAS_W,
  gridToScreen,
  hexToRgb,
  spriteName,
  themeOf,
  tileFrame,
  zIndexOf,
  type Kind,
  type LightName,
  type Theme,
} from "./floorplan.ts";
import {
  loadOfficeAssets,
  loadOwls,
  placeSprite,
  tintedTexture,
  type OfficeAssets,
} from "./sprites.ts";

const MINI_W = 120;
const MINI_H = 74;
const LIGHT_MS = 1500;
const RESIZE_DEBOUNCE_MS = 100;

/**
 * 배율은 장치 픽셀 기준 정수 (PLAN 3.2). 백킹은 장치 픽셀, CSS 크기는 소수여도 된다.
 * 사용 가능 크기도 장치 픽셀로 받는다. 반올림한 CSS px 에 DPR 을 곱하면 실제보다 커져 배율이 한 단계 올라간다.
 */
export function canvasScale(availDevW: number, availDevH: number, dpr: number) {
  const scale = Math.max(1, Math.floor(Math.min(availDevW / CANVAS_W, availDevH / CANVAS_H)));
  return {
    scale,
    backingW: CANVAS_W * scale,
    backingH: CANVAS_H * scale,
    cssW: (CANVAS_W * scale) / dpr,
    cssH: (CANVAS_H * scale) / dpr,
  };
}

/**
 * 패닝은 축마다 캔버스(백킹)가 무대(장치 픽셀)보다 클 때만 열린다 (HANDOFF 2.2, 11).
 * 무대는 가운데 정렬이라 넘친 만큼 양쪽으로 반씩 나간다. 반환은 CSS px.
 */
export function panLimits(
  backingW: number,
  backingH: number,
  availDevW: number,
  availDevH: number,
  dpr: number,
): { maxX: number; maxY: number } {
  return {
    maxX: Math.max(0, backingW - availDevW) / 2 / dpr,
    maxY: Math.max(0, backingH - availDevH) / 2 / dpr,
  };
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
  /** depth 로 정렬하는 월드. 직원 액터가 여기 들어간다 */
  world: Container;
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

  // 장애 빛은 평면도 기본이 숨김이고 장애 중에만 보인다 (참조 make_canvas_images 와 같다)
  const placements = fp.layouts[kind].placements.filter(
    (pl) => pl.visible !== false || (incident && pl.id === "glow_incident"),
  );
  // 깔개·벽 소품은 바닥 위에 배열 순서로, 월드는 depth 로 (render_scene 3·4단계)
  placements.forEach((pl, seq) => {
    const key = `${spriteName(pl, theme)}/${pl.state}`;
    // ponytail: 중소기업 좌석의 팀 색은 직원을 앉히는 T20 에서 넣는다, 지금은 빈자리 색(테마 cubicle)만
    const tint = pl.id.startsWith("dev_part_") ? fp.themes[theme].cubicle : undefined;
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
  return { root, lit: [base, world], world };
}

/** store 에서 렌더러가 읽는 부분만. */
interface OfficeStore {
  state: { list: OfficeListDto | null; selectedId: string | null; office: OfficeDto | null };
  subscribe(fn: () => void): () => void;
}

type Rgb = [number, number, number];
const WHITE: Rgb = [255, 255, 255];

/** `stage` 는 가운데 무대(`.ow-stage`). 캔버스·방 이름표·미니맵을 그 안에 만든다. */
export async function mountOffice(stage: HTMLElement, store: OfficeStore): Promise<void> {
  const app = await createOfficeApp();
  const assets = await loadOfficeAssets();
  const fp = assets.floorplan;
  const owls = await loadOwls().catch((e) => {
    console.error("owls.json 을 못 읽어 대체 몸체로 그린다", e);
    return null;
  });

  const wrap = document.createElement("div");
  wrap.className = "ow-canvas-wrap";
  const overlay = document.createElement("div");
  overlay.className = "ow-overlay";
  // 직원 이름표·Zzz 는 방 이름표 위, 장면을 새로 지어도 지워지지 않게 따로 둔다
  const actorOverlay = document.createElement("div");
  actorOverlay.className = "ow-overlay";
  wrap.append(app.canvas, overlay, actorOverlay);
  const actors = new ActorLayer(fp, assets.sheet, owls, actorOverlay);
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
  let officeId = "";
  let labels: { el: HTMLElement; x: number; y: number }[] = [];
  let fit = canvasScale(CANVAS_W, CANVAS_H, 1);
  let dpr = 1;
  // 무대의 사용 가능 크기, 장치 픽셀
  let avail = { w: CANVAS_W, h: CANVAS_H };
  const pan = { x: 0, y: 0, maxX: 0, maxY: 0 };

  // ---------- 조명: 바뀌면 1.5초 보간 ----------
  let lightFrom: Rgb = WHITE;
  let lightTo: Rgb = WHITE;
  let lightCur: Rgb = WHITE;
  let lightT = 1;
  function setLight(name: LightName | null, immediate: boolean): void {
    const to = name ? hexToRgb(fp.lightTints[name]) : WHITE;
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
    // ponytail: 선택 응답이 끝내 안 오면 옛 사무실이 남는다, 사무실 전환 티켓(T28)에서 상한을 정한다
    if (!office && selectedId !== null && scene) return;
    // 규모는 office 에서 읽는다. 설정 전환과 늦은 선택 응답이 겹쳐도 small 평면도에 대기업 팀이 섞이지 않는다
    const kind: Kind = office
      ? office.officeId.startsWith("vendor/")
        ? "small"
        : "large"
      : list.scale === "Small"
        ? "small"
        : "large";
    const theme = themeOf(office?.teams[0]?.building);
    const incident = office?.shared.incident ?? false;
    const key = `${kind}|${theme}|${incident}`;
    const switched = !scene || (office?.officeId ?? "") !== officeId;
    if (key !== sceneKey) {
      // 새 장면을 다 지은 뒤에 옛것을 지운다. 짓다 던져도 화면은 옛 장면이 남는다
      const next = buildScene(assets, kind, theme, incident);
      actors.attach(next.world);
      scene?.root.destroy({ children: true });
      scene = next;
      app.stage.addChild(scene.root);
      sceneKey = key;
      buildLabels(kind);
    }
    officeId = office?.officeId ?? "";
    actors.sync(office, kind, Date.now());
    // ponytail: dim(30분 조용 + 프로세스 살아 있음, S58)은 DTO 에 판정 필드가 없어 아직 안 켠다. 필드가 생기면 넣는다
    const off = !office || office.teams.every((t) => t.phase === "Off");
    setLight(off ? "off" : office.shared.night ? "night" : null, switched);
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
    fit = canvasScale(avail.w, avail.h, dpr);
    if (app.renderer.width !== fit.backingW || app.renderer.height !== fit.backingH)
      app.renderer.resize(fit.backingW, fit.backingH);
    app.stage.scale.set(fit.scale);
    for (const el of [wrap, app.canvas]) {
      el.style.width = `${fit.cssW}px`;
      el.style.height = `${fit.cssH}px`;
    }
    placeLabels();
    actors.placeOverlay((x, y) => toCss(x, y, fit.scale, dpr));
    const lim = panLimits(fit.backingW, fit.backingH, avail.w, avail.h, dpr);
    pan.maxX = lim.maxX;
    pan.maxY = lim.maxY;
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
    const stageW = avail.w / dpr;
    const stageH = avail.h / dpr;
    const left = (stageW - fit.cssW) / 2 + pan.x;
    const top = (stageH - fit.cssH) / 2 + pan.y;
    const x0 = Math.max(0, -left);
    const y0 = Math.max(0, -top);
    const x1 = Math.min(fit.cssW, stageW - left);
    const y1 = Math.min(fit.cssH, stageH - top);
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

  // 소수 CSS px 를 반올림하지 않도록 장치 픽셀 크기를 그대로 받는다
  function measure(): void {
    const r = stage.getBoundingClientRect();
    const d = window.devicePixelRatio || 1;
    avail = { w: Math.floor(r.width * d), h: Math.floor(r.height * d) };
  }
  let timer: ReturnType<typeof setTimeout> | undefined;
  // device-pixel-content-box: CSS 크기는 그대로인데 DPR 만 바뀌어도(모니터 이동, 배율 변경) 다시 맞춘다
  new ResizeObserver((entries) => {
    const box = entries[entries.length - 1]?.devicePixelContentBoxSize?.[0];
    if (box) avail = { w: box.inlineSize, h: box.blockSize };
    else measure();
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
        // 던지면 Pixi Ticker 가 멈춰 캔버스가 영원히 빈다. 알리고 다음 틱으로
        try {
          apply();
        } catch (e) {
          console.error("사무실 장면 적용 실패", e);
        }
      }
      if (lightT < 1) stepLight(t.deltaMS);
      actors.update(Date.now());
    },
    undefined,
    UPDATE_PRIORITY.HIGH,
  );
  // 앱의 렌더(LOW) 뒤라 WebGL 버퍼가 아직 살아 있다 (preserveDrawingBuffer 없이 복사)
  app.ticker.add(
    () => {
      if (!minimap.hidden && miniCtx) {
        miniCtx.clearRect(0, 0, MINI_W, MINI_H);
        miniCtx.drawImage(app.canvas, 0, 0, MINI_W, MINI_H);
      }
    },
    undefined,
    UPDATE_PRIORITY.UTILITY,
  );
  document.addEventListener("visibilitychange", () => {
    if (document.hidden) app.ticker.stop();
    else app.ticker.start();
  });
  measure();
  layout();
}
