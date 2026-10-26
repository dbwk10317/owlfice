// 사무실 캔버스: PixiJS 초기화, 정수 배율, 그리기 순서, 패닝·미니맵, 방 이름표 (HANDOFF 6.1, 6.2, 4.4, 2.2).
import {
  Application,
  Container,
  Polygon,
  TextureStyle,
  UPDATE_PRIORITY,
  type Sprite,
  type Texture,
} from "pixi.js";
// 배포 CSP(default-src 'self')는 eval 을 막는데 Pixi 렌더러는 생성 때 new Function 을 시험하고 던진다. eval 없는 길로 바꿔 둔다
import "pixi.js/unsafe-eval";
import phrases from "../../design/handoff/phrases.ko.json";
import { fill } from "../text.ts";
import { ActorLayer, seatProps, type Owls } from "./actors.ts";
import type { OfficeSound } from "./sound.ts";
import { DRIVEN_PROP, PropsLayer, ScreenLabel, loadWallFont } from "./props.ts";
import { LADDER_MS } from "./anim-queue.ts";
import { CanvasTooltip, propTooltip, roomTooltip } from "./canvas-tooltip.ts";
import type { CueDto, OfficeDto, OfficeListDto, TeamDto } from "../types/ipc.ts";
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
  sheetDensity,
  tintedTexture,
  type Animal,
  type Density,
  type OfficeAssets,
} from "./sprites.ts";

const MINI_W = 120;
const MINI_H = 74;
const LIGHT_MS = 1500;
const RESIZE_DEBOUNCE_MS = 100;
const DRAG_PX = 4;

/**
 * 배율은 장치 픽셀 기준 정수 (design/HANDOFF.md 6.1). 백킹은 장치 픽셀, CSS 크기는 소수여도 된다.
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

/** 창에 맞는 가장 큰 배율. 720 미만 창은 1× 고정에 패닝 (HANDOFF 2.2, 6.1). */
export function maxFitScale(
  availDevW: number,
  availDevH: number,
  dpr: number,
  viewportW: number,
): number {
  return viewportW < 720 ? 1 : canvasScale(availDevW, availDevH, dpr).scale;
}

/**
 * 직원 시트 교체 (HANDOFF 6.4). 요청이 겹치면 (동물, 밀도) 마지막 것만 적용하고, 로드 중엔 지금 시트를 그대로 그린다.
 * 실패하면 알리고 지금 시트를 유지한다. 같은 요청이 다시 오면 그때 다시 읽는다.
 */
export function sheetSwitcher(
  animal: Animal,
  density: Density,
  load: (animal: Animal, density: Density) => Promise<Owls>,
  apply: (owls: Owls) => void,
): (animal: Animal, density: Density) => void {
  let shown = `${animal}@${density}`;
  let wanted = shown;
  return (animal, density) => {
    const key = `${animal}@${density}`;
    if (key === wanted) return;
    wanted = key;
    // 지금 그리는 시트로 돌아왔으면 읽고 있던 것은 끝나도 버린다
    if (key === shown) return;
    load(animal, density).then(
      (owls) => {
        if (wanted !== key) return;
        shown = key;
        apply(owls);
      },
      (e) => {
        console.error(`직원 시트 ${key} 를 못 읽어 지금 시트를 유지한다`, e);
        if (wanted === key) wanted = shown;
      },
    );
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
  overlay: Container;
  /** 평면도가 숨겨 둔 채 짓고 좌석 사용에 따라 켜는 소품 (임시·간이 책상, 쪽지) */
  toggled: Map<string, Sprite>;
  /** 소품 상태가 바꾸는 평면도 소품 (신호등, 창, 화분, 휴지통, 전광판, 컵라면) */
  driven: Map<string, Sprite>;
  /** 눌러서 세션 상세를 여는 소품 (사규 액자, 팀장석 책상) */
  clickable: Map<string, Sprite>;
  rooms: Map<string, Container>;
  /** 칸반 포스트잇·TV 선이 얹히는 벽 컨테이너 */
  wall: Container;
}

const SESSION_PROP = /^(rules_frame|boss_desk_\d+)$/;

/**
 * 눌린 소품이 가리키는 팀. 사규 액자는 포커스 팀 또는 최근 활동 팀 것(docs/CONTRACTS.md C8), 팀장석은 그 자리(deskSlot)의 팀이다.
 * 명패는 따로 그리는 소품이 없어 팀장석 책상을 누르게 한다.
 */
export function teamOfProp(
  id: string,
  office: OfficeDto | null,
  focusedTeam: string | null = null,
): TeamDto | undefined {
  const teams = office?.teams ?? [];
  const desk = /^boss_desk_(\d+)$/.exec(id);
  if (desk) return teams.find((t) => t.deskSlot === Number(desk[1]));
  if (id !== "rules_frame") return undefined;
  const focused = teams.find((t) => t.teamId === focusedTeam);
  if (focused && office?.officeId.startsWith("vendor/")) return focused;
  // 동점이면 뒤 팀이 이긴다 (백엔드 frame_title 의 max_by_key 와 같다)
  // ponytail: Date.parse 는 ms 까지만 봐서 그 밑 차이는 동점이 된다, 백엔드와 어긋나는 사례가 나오면 DTO 에 순번을 얹는다
  const at = (t: TeamDto) => Date.parse(t.lastActivity) || 0;
  return teams.reduce<TeamDto | undefined>((a, t) => (!a || at(t) >= at(a) ? t : a), undefined);
}

// 평면도에서 visible:false 이고 직원이 앉으면 보이는 소품 (HANDOFF 6.5)
const SEAT_PROP = /^(temp_desk_|spare_desk_)\d+$|^note_temp$/;

/** render_scene 의 그리기 순서 그대로. 소품은 평면도의 기본 상태, 상태는 PropsLayer 가 바꾼다. */
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
  const rooms = new Map<string, Container>();
  for (const [id, room] of Object.entries(fp.rooms)) {
    const [x, y, w, h] = room.rect;
    const points = [
      [x, y],
      [w, y],
      [w, h],
      [x, h],
    ].flatMap(([gx, gy]) => {
      const p = gridToScreen(gx, gy);
      return [p.x, p.y];
    });
    const hit = new Container({ eventMode: "static", cursor: "pointer" });
    hit.hitArea = new Polygon(points);
    base.addChild(hit);
    rooms.set(id, hit);
  }

  // 장애 빛은 평면도 기본이 숨김이고 장애 중에만 보인다 (참조 make_canvas_images 와 같다)
  const placements = fp.layouts[kind].placements.filter(
    (pl) =>
      pl.visible !== false ||
      (incident && pl.id === "glow_incident") ||
      SEAT_PROP.test(pl.id) ||
      DRIVEN_PROP.test(pl.id),
  );
  const toggled = new Map<string, Sprite>();
  const driven = new Map<string, Sprite>();
  const clickable = new Map<string, Sprite>();
  // 깔개·벽 소품은 바닥 위에 배열 순서로, 월드는 depth 로 (render_scene 3·4단계)
  placements.forEach((pl, seq) => {
    const key = `${spriteName(pl, theme)}/${pl.state}`;
    const tint = pl.id.startsWith("dev_part_") ? fp.themes[theme].cubicle : undefined;
    const place = (k: string, texture?: Texture) => {
      const s = placeSprite(sheet, k, pl.x, pl.y, texture);
      if (pl.layer === "decal" || pl.layer === "wall") base.addChild(s);
      else if (pl.layer === "window") win.addChild(s);
      else if (pl.layer === "overlay") over.addChild(s);
      else {
        s.zIndex = zIndexOf(pl.depth, seq);
        world.addChild(s);
      }
      return s;
    };
    const s = place(key, tint ? tintedTexture(sheet, key, tint) : undefined);
    if (SESSION_PROP.test(pl.id)) clickable.set(pl.id, s);
    if (DRIVEN_PROP.test(pl.id)) {
      driven.set(pl.id, s);
      if (pl.visible === false) s.visible = false;
    }
    if (SEAT_PROP.test(pl.id)) {
      s.visible = false;
      toggled.set(pl.id, s);
    }
    // 만석 쪽지는 임시 책상 쪽지 자리에 놓고 그것을 대신한다 (평면도에 따로 없다)
    if (pl.id === "note_temp") {
      const full = place("note_full/default");
      full.visible = false;
      toggled.set("note_full", full);
    }
  });

  const root = new Container();
  root.addChild(base, win, world, over);
  return {
    root,
    lit: [base, world],
    world,
    overlay: over,
    toggled,
    driven,
    clickable,
    rooms,
    wall: base,
  };
}

/**
 * 틱 사이에 office-state 가 둘 이상 오면 sync 는 마지막 DTO 만 본다. 앞 DTO 의 cue 를 모아 두었다가 다음 sync 에 넘긴다.
 * 이미 sync 에 넘긴 DTO 의 cue 는 다시 넘기지 않는다.
 * 창이 숨었을 땐 모은 것을 다 버리고 모으지 않는다. 틱이 멈춰 take 가 없고, 돌아온 첫 sync 는 cue 를 버리니
 * 숨기 전 DTO 가 그 뒤 sync 에 끼어들면 안 된다 (docs/CONTRACTS.md C10).
 */
export function cueCollector() {
  let earlier: CueDto[] = [];
  let latest: OfficeDto | null = null;
  let handled: OfficeDto | null = null;
  return {
    see(o: OfficeDto | null, hidden: boolean): void {
      if (hidden) {
        earlier = [];
        latest = null;
        handled = null;
        return;
      }
      if (!o || o === latest) return;
      if (latest && latest !== handled) earlier.push(...latest.cues);
      latest = o;
    },
    take(o: OfficeDto | null): CueDto[] {
      const out = earlier;
      earlier = [];
      if (o) handled = o;
      return out;
    },
  };
}

/** store 에서 렌더러가 읽는 부분만. */
interface OfficeStore {
  state: { list: OfficeListDto | null; selectedId: string | null; office: OfficeDto | null };
  subscribe(fn: () => void): () => void;
}

type Rgb = [number, number, number];
const WHITE: Rgb = [255, 255, 255];

/** 머리말이 부르는 것 */
export interface OfficeHandle {
  setScale(scale: number): void;
  centerCanvas(): void;
  setRoomLabels(on: boolean): void;
  setBossName(name: string | null): void;
  highlightTeam(officeId: string, teamId: string): void;
  /** 설정의 직원 동물. 지금 배율의 밀도로 시트를 다시 읽는다 */
  setAnimal(animal: Animal): void;
}
export interface OfficeOptions {
  /** 직원 동물 (기본 올빼미) */
  animal?: Animal;
  season?: boolean;
  sound?: OfficeSound;
  bossName?: string | null;
  /** 장치 픽셀 기준. 현재 배율 또는 최대맞춤 배율이 바뀔 때 알린다 */
  onScale?(scale: number, maxFit: number): void;
  /** 직원을 눌렀을 때 (직원 상세 팝업). officeId 는 캔버스에 그려진 사무실이다: 전환 중엔 선택과 다를 수 있다 */
  onEmployee?: (officeId: string, teamId: string, agentId: string) => void;
  /** 사규 액자·팀장석을 눌렀을 때 (세션 상세 팝업) */
  onSession?: (officeId: string, teamId: string) => void;
}

/** `stage` 는 가운데 무대(`.ow-stage`). 캔버스·방 이름표·미니맵을 그 안에 만든다. */
export async function mountOffice(
  stage: HTMLElement,
  store: OfficeStore,
  opts: OfficeOptions = {},
): Promise<OfficeHandle> {
  const app = await createOfficeApp();
  await loadWallFont();
  const assets = await loadOfficeAssets();
  const fp = assets.floorplan;
  let animal: Animal = opts.animal ?? "owls";
  // 첫 layout() 과 같은 배율의 밀도로 읽는다. 아직 고른 배율은 없다
  const dpr0 = window.devicePixelRatio || 1;
  const box0 = stage.getBoundingClientRect();
  const density0 = sheetDensity(
    maxFitScale(
      Math.floor(box0.width * dpr0),
      Math.floor(box0.height * dpr0),
      dpr0,
      window.innerWidth,
    ),
  );
  const owls = await loadOwls(animal, density0).catch((e) => {
    console.error("직원 시트를 못 읽어 대체 몸체로 그린다", e);
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
  // 이름표보다 아래에 깔리게 먼저 만든다
  const screenLabel = new ScreenLabel(actorOverlay);
  const actors = new ActorLayer(fp, assets.sheet, owls, actorOverlay);
  // 대체 몸체로 시작했으면 시트를 바꾸지 않는다 (ActorLayer.setOwls)
  const pickSheet = owls
    ? sheetSwitcher(animal, density0, loadOwls, (next) => actors.setOwls(next))
    : () => {};
  let bossName = opts.bossName || phrases.ranks.boss;
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
  const tooltip = new CanvasTooltip(stage);
  const miniCtx = miniCanvas.getContext("2d");

  let scene: Scene | null = null;
  let props: PropsLayer | null = null;
  let shown: OfficeDto | null = null;
  let propsAt = 0;
  let propsTeam: string | null = null;
  let pendingTeam: { officeId: string; teamId: string } | null = null;
  let sceneKey = "";
  let officeId = "";
  let switchFade: Animation | undefined;
  const reducedMotion = window.matchMedia("(prefers-reduced-motion: reduce)");
  reducedMotion.addEventListener("change", () => {
    switchFade?.cancel();
    resumed = true;
    dirty = true;
  });
  let suppressClick = false;
  actors.onPick = (teamId, agentId) => {
    if (!suppressClick) {
      tooltip.close();
      opts.onEmployee?.(officeId, teamId, agentId);
    }
  };
  actors.onBossPick = (teamId, x, y) => {
    const message = shown?.teams.find((t) => t.teamId === teamId)?.props.bossVisit;
    if (!suppressClick && message) showAt(`${bossName}\n${message}`, x, y);
  };
  actors.onRing = (now, cue) => {
    props?.ring(now);
    opts.sound?.cue(cue);
  };
  actors.onArchiveVisit = (active) => props?.archiveVisit(active);
  actors.onDoor = (now) => props?.openDoor(now);
  actors.onCue = (c, now, o) => {
    props?.cue(c, now, o);
    if (c.kind !== "BossVisit") opts.sound?.cue(c);
  };
  actors.onLadder = (at, now, title) => {
    props?.ladder(at, now);
    props?.deferTitle(title, at + LADDER_MS);
  };
  let labels: { el: HTMLElement; x: number; y: number }[] = [];
  let propPicks: { el: HTMLButtonElement; x: number; y: number; w: number; h: number }[] = [];
  let fit = canvasScale(CANVAS_W, CANVAS_H, 1);
  let dpr = 1;
  // 무대의 사용 가능 크기, 장치 픽셀
  let avail = { w: CANVAS_W, h: CANVAS_H };
  const pan = { x: 0, y: 0, maxX: 0, maxY: 0 };
  let notifiedScale = 0; // 0 이라 첫 layout() 이 현재 값을 알린다
  let notifiedMaxFit = 0;
  let preferredScale: number | null = null;

  function showAt(text: string, x: number, y: number): void {
    const rect = wrap.getBoundingClientRect();
    const p = toCss(x, y, fit.scale, dpr);
    tooltip.show(text, rect.left + p.left, rect.top + p.top);
  }

  function showRoom(id: string): void {
    const r = fp.rooms[id];
    if (suppressClick || !r || !shown) return;
    showAt(roomTooltip(id, r.name, shown, Date.now()), r.label[0], r.label[1]);
  }

  // ---------- 조명: 바뀌면 1.5초 보간 ----------
  let lightFrom: Rgb = WHITE;
  let lightTo: Rgb = WHITE;
  let lightCur: Rgb = WHITE;
  let lightT = 1;
  function setLight(name: LightName | null, immediate: boolean): void {
    const to = name ? hexToRgb(fp.lightTints[name]) : WHITE;
    if (!immediate && to.every((v, i) => v === lightTo[i])) return;
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

  // 값이 같으면 안 건드린다: 스냅샷마다 스크린리더가 다시 읽지 않게
  function setCanvasAria(office: OfficeDto | null, list: OfficeListDto): void {
    const el = app.canvas;
    const label = canvasAria(office, list);
    // 이름 없는 img 는 "이미지"로만 읽히니 이름이 없으면 역할도 뺀다
    if (label === null) {
      el.removeAttribute("role");
      el.removeAttribute("aria-label");
      return;
    }
    el.setAttribute("role", "img");
    if (el.getAttribute("aria-label") !== label) el.setAttribute("aria-label", label);
  }

  // ---------- 스냅샷 적용 ----------
  function apply(): void {
    const { list, office, selectedId } = store.state;
    if (!list) return;
    // 사무실을 바꾸는 중(선택은 있고 office 는 아직 null)에는 테마가 잠깐 바뀌어 깜빡이지 않게 지금 화면을 둔다
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
    const fade =
      switched && shown !== null && office !== null && !resumed && !reducedMotion.matches;
    if (key !== sceneKey) {
      // 새 장면을 다 지은 뒤에 옛것을 지운다. 짓다 던져도 화면은 옛 장면이 남는다
      const next = buildScene(assets, kind, theme, incident);
      actors.attach(next.world);
      for (const [id, sprite] of next.clickable) {
        sprite.eventMode = "static";
        sprite.cursor = "pointer";
        sprite.on("pointertap", () => {
          if (suppressClick) return;
          tooltip.close();
          const team = teamOfProp(id, shown, actors.focusedTeam(Date.now()));
          if (team && shown) opts.onSession?.(shown.officeId, team.teamId);
        });
      }
      for (const [id, room] of next.rooms) room.on("pointertap", () => showRoom(id));
      props = new PropsLayer(fp, assets.sheet, kind, next, screenLabel, opts.season ?? true);
      scene?.root.destroy({ children: true });
      scene = next;
      app.stage.addChild(scene.root);
      sceneKey = key;
      buildLabels(kind);
    }
    officeId = office?.officeId ?? "";
    if (switched || resumed || office?.rebuilt) {
      props?.reset();
      tooltip.close();
    }
    // 사무실을 바꾼 첫 스냅샷, 숨은 창에서 돌아온 스냅샷은 걷지 않고 제자리 (cue 도 버린다)
    actors.sync(office, kind, Date.now(), {
      instant: switched || resumed,
      earlier: cues.take(office),
    });
    const seated = seatProps(fp, kind, office);
    for (const [id, s] of scene?.toggled ?? []) s.visible = seated.has(id);
    shown = office;
    focusTeam();
    setCanvasAria(office, list);
    propsAt = Date.now();
    propsTeam = actors.focusedTeam(propsAt);
    props?.update(office, propsAt, propsTeam);
    setLight(lightOf(office), switched || resumed || reducedMotion.matches);
    resumed = false;
    stepLight(0);
    if (switched) {
      switchFade?.cancel();
      if (fade)
        switchFade = wrap.animate([{ opacity: 0 }, { opacity: 1 }], {
          duration: 300,
          easing: getComputedStyle(wrap).getPropertyValue("--ow-ease").trim() || "ease",
        });
    }
  }

  function focusTeam(): void {
    if (!pendingTeam) return;
    if (store.state.selectedId !== pendingTeam.officeId) {
      pendingTeam = null;
      return;
    }
    if (shown?.officeId !== pendingTeam.officeId || store.state.office !== shown) return;
    actors.highlightTeam(pendingTeam.teamId, Date.now());
    pendingTeam = null;
  }

  function buildLabels(kind: Kind): void {
    overlay.replaceChildren();
    const names = phrases.rooms as Record<string, string>;
    labels = Object.entries(fp.rooms).map(([k, room]) => {
      const el = document.createElement("button");
      el.type = "button";
      el.tabIndex = -1;
      el.className = "ow-roomlabel ow-canvas-pick";
      el.textContent = (kind === "small" && k === "boss" ? names.bossSmall : names[k]) ?? room.name;
      overlay.append(el);
      el.addEventListener("click", () => showRoom(k));
      return { el, x: room.label[0], y: room.label[1] };
    });
    for (const p of propPicks) p.el.remove();
    propPicks = fp.layouts[kind].placements
      .filter((p) => ["clock", "attendance", "stats_board"].includes(p.id))
      .map((p) => {
        const el = document.createElement("button");
        el.type = "button";
        el.tabIndex = -1;
        el.className = "ow-prop-pick ow-canvas-pick";
        el.setAttribute(
          "aria-label",
          { clock: "벽시계", attendance: "근태판", stats_board: "오늘의 통계" }[p.id]!,
        );
        const frame = assets.sheet.data.frames[`${p.sprite}/${p.state}`] as {
          frame: { w: number; h: number };
          anchorPx?: number[];
        };
        const [ax, ay] = frame.anchorPx ?? [0, 0];
        const open = () => {
          if (suppressClick) return;
          const r = el.getBoundingClientRect();
          tooltip.show(propTooltip(p.id, shown, Date.now()), r.right, r.bottom, el);
        };
        el.addEventListener("pointerenter", open);
        el.addEventListener("focus", open);
        el.addEventListener("click", open);
        actorOverlay.append(el);
        return { el, x: p.x - ax, y: p.y - ay, w: frame.frame.w, h: frame.frame.h };
      });
    placeLabels();
  }

  function placeLabels(): void {
    for (const l of labels) {
      const p = toCss(l.x, l.y, fit.scale, dpr);
      l.el.style.left = `${p.left}px`;
      l.el.style.top = `${p.top}px`;
    }
    for (const p of propPicks) {
      const at = toCss(p.x, p.y, fit.scale, dpr);
      const size = toCss(p.w, p.h, fit.scale, dpr);
      Object.assign(p.el.style, {
        left: `${at.left}px`,
        top: `${at.top}px`,
        width: `${size.left}px`,
        height: `${size.top}px`,
      });
    }
  }

  // ---------- 배율·패닝 ----------
  function layout(): void {
    tooltip.close();
    dpr = window.devicePixelRatio || 1;
    const maxFit = maxFitScale(avail.w, avail.h, dpr, window.innerWidth);
    const scale = Math.min(preferredScale ?? maxFit, maxFit);
    fit = canvasScale(CANVAS_W * scale, CANVAS_H * scale, dpr);
    if (app.renderer.width !== fit.backingW || app.renderer.height !== fit.backingH)
      app.renderer.resize(fit.backingW, fit.backingH);
    app.stage.scale.set(fit.scale);
    pickSheet(animal, sheetDensity(fit.scale));
    if (fit.scale !== notifiedScale || maxFit !== notifiedMaxFit) {
      notifiedScale = fit.scale;
      notifiedMaxFit = maxFit;
      opts.onScale?.(fit.scale, maxFit);
    }
    for (const el of [wrap, app.canvas]) {
      el.style.width = `${fit.cssW}px`;
      el.style.height = `${fit.cssH}px`;
    }
    placeLabels();
    const css = (x: number, y: number) => toCss(x, y, fit.scale, dpr);
    actors.placeOverlay(css);
    screenLabel.place(css);
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

  let drag: { id: number; sx: number; sy: number; x0: number; y0: number; moved: boolean } | null =
    null;
  wrap.addEventListener("pointerdown", (e) => {
    suppressClick = false;
    drag = {
      id: e.pointerId,
      sx: e.clientX - pan.x,
      sy: e.clientY - pan.y,
      x0: e.clientX,
      y0: e.clientY,
      moved: false,
    };
  });
  wrap.addEventListener("pointermove", (e) => {
    if (drag?.id !== e.pointerId) return;
    // 캡처 전에 밖에서 놓으면 pointerup 을 못 받아 drag 가 남는다
    if (e.buttons === 0) {
      drag = null;
      return;
    }
    // 끌기로 확정되기 전에 캡처하면 Pixi 가 pointerup 을 못 받아 직원·소품 클릭이 죽는다
    if (!drag.moved) {
      if (Math.hypot(e.clientX - drag.x0, e.clientY - drag.y0) < DRAG_PX) return;
      drag.moved = true;
      suppressClick = true;
      tooltip.close();
      if (!minimap.hidden) wrap.setPointerCapture(e.pointerId);
    }
    movePan(e.clientX - drag.sx, e.clientY - drag.sy);
  });
  const endDrag = (e: PointerEvent) => {
    if (drag?.id === e.pointerId) {
      drag = null;
      // pointerup 뒤 이어지는 click까지 막고, 다음 hover는 다시 연다.
      setTimeout(() => {
        suppressClick = false;
      }, 0);
    }
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
  // 스냅샷은 표시만 해 두고 틱에서 한 번 적용한다. 숨은 창에선 틱이 멈춰 마지막 것만 보일 때 적용된다 (docs/CONTRACTS.md C10)
  let dirty = true;
  let resumed = false;
  const cues = cueCollector();
  const soundState = () =>
    opts.sound?.see(store.state.office, store.state.selectedId, document.hidden);
  soundState();
  store.subscribe(() => {
    soundState();
    if (store.state.selectedId !== officeId) actors.highlightTeam(null, Date.now());
    if (store.state.selectedId !== officeId) tooltip.close();
    dirty = true;
    if (store.state.selectedId !== officeId) switchFade?.cancel();
    cues.see(store.state.office, document.hidden);
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
      const now = Date.now();
      // 무재해 분과 창밖 시각은 상태가 안 와도 흐른다. 바뀐 소품만 다시 그린다
      const focused = actors.focusedTeam(now);
      if (now - propsAt >= 1000 || focused !== propsTeam) {
        propsAt = now;
        propsTeam = focused;
        props?.update(shown, now, focused);
      }
      props?.tick(now);
      actors.update(now);
      opts.sound?.keyboard(actors.isTyping(), now);
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
    soundState();
    switchFade?.cancel();
    if (document.hidden) app.ticker.stop();
    else {
      resumed = true;
      dirty = true;
      app.ticker.start();
    }
  });
  measure();
  layout();
  return {
    setScale: (scale) => {
      if (!Number.isInteger(scale) || scale < 1 || scale > 3) return;
      preferredScale = scale;
      layout();
    },
    centerCanvas: () => movePan(0, 0),
    setRoomLabels: (on) => void (overlay.hidden = !on),
    setBossName: (name) => {
      bossName = name || phrases.ranks.boss;
    },
    highlightTeam: (officeId, teamId) => {
      pendingTeam = { officeId, teamId };
      focusTeam();
    },
    setAnimal: (next) => {
      animal = next;
      pickSheet(animal, sheetDensity(fit.scale));
    },
  };
}

/** 캔버스 aria-label (HANDOFF 10절): 그린 사무실의 이름·직원 수·장애. 사무실이 없으면 null */
export function canvasAria(office: OfficeDto | null, list: OfficeListDto): string | null {
  if (!office) return null;
  const name = list.offices.find((o) => o.officeId === office.officeId)?.label;
  if (!name) return null;
  const n = office.teams.reduce((sum, t) => sum + t.employees.length, 0);
  const tpl = office.shared.incident ? phrases.canvas.ariaIncident : phrases.canvas.aria;
  return fill(tpl, { office: name, n });
}

/** 퇴근·대기 조명이 시간표보다 우선한다 (HANDOFF 6.2). */
export function lightOf(office: OfficeDto | null): LightName | null {
  const live = office?.teams.filter((t) => t.phase !== "Off") ?? [];
  if (live.length === 0) return "off";
  if (live.every((t) => t.idleAlive)) return "dim";
  const daylight = live[0].daylight;
  return daylight === "day" ? null : daylight;
}
