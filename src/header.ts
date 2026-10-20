// 가운데 머리말 (HANDOFF 4.3)과 창 너비 구간 (PLAN 3.2). store 가 바뀌면 바뀐 칸만 고친다.
import "../design/handoff/tokens.css";
import "./styles/app.css";
import phrases from "../design/handoff/phrases.ko.json";
import nightSvg from "../design/handoff/icons/night.svg?raw";
import panelSvg from "../design/handoff/icons/panel.svg?raw";
import roomLabelsSvg from "../design/handoff/icons/room-labels.svg?raw";
import type { Viewport } from "./sidebar-right.ts";
import type { OfficeDto, OfficeListDto, TeamSummaryDto, WorkType } from "./types/ipc.ts";

const H = phrases.header;
const TICK_MS = 30_000;

export type Layout = "wide" | "drawer" | "rail" | "select";
/** 3.2 표의 너비 구간. 사이드바(900/720)와 현황판(900) 경계가 같다 */
export function layoutOf(width: number): Layout {
  return width >= 1100 ? "wide" : width >= 900 ? "drawer" : width >= 720 ? "rail" : "select";
}

export function windowViewport(win: Window): Viewport {
  return {
    size: () => ({ w: win.innerWidth, h: win.innerHeight }),
    subscribe(cb) {
      let t: ReturnType<typeof setTimeout> | undefined;
      const on = () => {
        clearTimeout(t);
        t = setTimeout(cb, 100);
      };
      win.addEventListener("resize", on);
      return () => {
        clearTimeout(t);
        win.removeEventListener("resize", on);
      };
    },
  };
}

// 동 글자 = 제품 (PLAN 2부). 사이드바와 같은 표
const BUILDING: Record<string, [string, string]> = {
  A: ["Claude Desktop", "claude"],
  B: ["Claude Code CLI", "claude"],
  C: ["Codex 앱", "codex"],
  D: ["Codex CLI", "codex"],
  E: ["Antigravity 앱", "gemini"],
  F: ["Antigravity CLI", "gemini"],
};
const FLOOR_PRODUCT = ["claude", "codex", "gemini"];
const WORK_TYPES: WorkType[] = ["CodeReview", "BugFix", "Refactor", "Docs", "Research", "NewDev"];

const fill = (tpl: string, vars: Record<string, string | number>) =>
  tpl.replace(/\{(\w+)\}/g, (_, k: string) => String(vars[k] ?? ""));

// "근무 중 · {elapsed}째" 의 elapsed. 1분 미만도 1분으로 센다
function elapsedText(startedMs: number, now: number): string {
  const min = Math.max(1, Math.floor((now - startedMs) / 60_000));
  const u = phrases.units;
  if (min < 60) return fill(u.min, { n: min });
  return min % 60 === 0
    ? `${min / 60}시간`
    : fill(u.hourMin, { h: Math.floor(min / 60), m: min % 60 });
}

const clock = (ms: number) => {
  const d = new Date(ms);
  return `${String(d.getHours()).padStart(2, "0")}:${String(d.getMinutes()).padStart(2, "0")}`;
};

export interface HeaderStore {
  state: { list: OfficeListDto | null; selectedId: string | null; office: OfficeDto | null };
  subscribe(fn: () => void): () => void;
}
export interface HeaderOptions {
  /** 창 크기 출처. 기본은 window 크기와 resize(100ms 디바운스) */
  viewport?: Viewport;
  now?: () => number;
  togglePanel(): void;
  setRoomLabels(on: boolean): void;
}
export interface HeaderHandle {
  roomLabels(): boolean;
  setScale(scale: number): void;
  setDrawerOpen(open: boolean): void;
  destroy(): void;
}

type El = HTMLElement;
function mk(doc: Document, tag: string, cls = "", text?: string): El {
  const e = doc.createElement(tag);
  if (cls) e.className = cls;
  if (text !== undefined) e.textContent = text;
  return e;
}
function iconBtn(doc: Document, svg: string, label: string): HTMLButtonElement {
  const b = doc.createElement("button");
  b.type = "button";
  b.className = "ow-iconbtn";
  b.setAttribute("aria-label", label);
  const i = mk(doc, "span", "ow-icon");
  i.setAttribute("aria-hidden", "true");
  i.innerHTML = svg; // 앱에 묶인 정적 아이콘이다
  b.append(i);
  return b;
}
const setText = (el: El, text: string) => {
  if (el.textContent !== text) el.textContent = text;
};

/**
 * `app` 은 `.ow-app`, `header` 는 `.ow-main` 안의 `<header>`. 창 너비 구간은 app.dataset.layout 으로 알린다.
 * 소리 토글·브랜치 칩은 받을 기능·데이터가 없어 만들지 않는다.
 */
export function mountHeader(
  app: El,
  header: El,
  store: HeaderStore,
  opts: HeaderOptions,
): HeaderHandle {
  const doc = header.ownerDocument;
  const now = opts.now ?? Date.now;
  header.classList.add("ow-header");

  const square = mk(doc, "span", "ow-header__square");
  const title = mk(doc, "h1", "ow-header__title");
  const sub = mk(doc, "span", "ow-header__sub ow-header__full");
  const slash = mk(doc, "span", "ow-header__sub ow-header__full", "/");
  const session = mk(doc, "span", "ow-header__session ow-header__full");
  const status = mk(doc, "span", "ow-chip ow-header__full");
  const statusDot = mk(doc, "span", "ow-dot");
  const statusNight = mk(doc, "span", "ow-icon ow-icon--sm");
  statusNight.setAttribute("aria-hidden", "true");
  statusNight.innerHTML = nightSvg;
  const statusText = mk(doc, "span", "ow-header__status");
  status.append(statusDot, statusNight, statusText);
  const work = mk(doc, "span", "ow-chip ow-header__work");
  const divider = mk(doc, "span", "ow-header__divider ow-header__full");
  const labels = iconBtn(doc, roomLabelsSvg, H.roomLabels);
  labels.classList.add("ow-header__full");
  labels.setAttribute("aria-pressed", "true");
  const scale = mk(doc, "span", "ow-chip ow-header__scale ow-header__full", "1×");
  scale.title = H.scale;
  const panelBtn = iconBtn(doc, panelSvg, H.panelOpen);
  panelBtn.classList.add("ow-iconbtn--solid", "ow-header__panel");
  panelBtn.setAttribute("aria-expanded", "false");
  const spacer = mk(doc, "span", "ow-header__spacer");
  header.append(
    square, title, sub, slash, session, status, work, spacer,
    divider, labels, scale, panelBtn,
  ); // prettier-ignore

  let roomLabelsOn = true;
  labels.addEventListener("click", () => {
    roomLabelsOn = !roomLabelsOn;
    labels.setAttribute("aria-pressed", String(roomLabelsOn));
    opts.setRoomLabels(roomLabelsOn);
  });
  panelBtn.addEventListener("click", () => opts.togglePanel());

  function render(): void {
    const { list, selectedId, office } = store.state;
    const sel = list?.offices.find((o) => o.officeId === selectedId);
    const large = list?.scale === "Large";
    const live: TeamSummaryDto[] = sel?.teams.filter((t) => t.phase !== "Off") ?? [];

    let accent = "var(--ow-ink-4)";
    let subText = "";
    if (sel && large) {
      const [name, key] = BUILDING[sel.building ?? ""] ?? ["", "codex"];
      accent = `var(--ow-${key})`;
      const host = sel.teams[0]?.hostBadge;
      subText = host ? `${name} · ${host}` : name;
    } else if (sel) {
      accent = `var(--ow-${FLOOR_PRODUCT[(sel.floor ?? 1) - 1] ?? "codex"})`;
      const people = live.reduce((s, t) => s + 1 + t.others, 0);
      subText = fill(phrases.sidebar.floorMeta, { teams: live.length, people });
    }
    square.style.setProperty("--ow-square", accent);
    setText(title, sel?.label ?? H.hq);
    setText(sub, subText);
    sub.hidden = !subText;
    const sessionTitle = large ? (office?.teams[0]?.title ?? sel?.teams[0]?.title ?? "") : "";
    setText(session, sessionTitle);
    slash.hidden = session.hidden = !sessionTitle;

    const t = now();
    const night = !!office?.shared.night;
    const off = !sel || live.length === 0;
    status.hidden = !sel;
    statusDot.hidden = night;
    statusNight.hidden = !night;
    statusDot.dataset.state = off ? "off" : "working";
    if (sel) {
      const started = Math.min(...live.map((x) => Date.parse(x.started)));
      setText(
        statusText,
        off
          ? fill(H.allOff, { time: clock(t) })
          : night
            ? fill(H.night, { time: clock(t) })
            : fill(H.working, { elapsed: elapsedText(started, t) }),
      );
    }

    const wt = live[0]?.workType;
    work.hidden = !wt;
    if (wt)
      setText(work, fill(H.workType, { type: phrases.workType[WORK_TYPES.indexOf(wt)] ?? "" }));
  }

  const view = opts.viewport ?? windowViewport(doc.defaultView!);
  function onResize(): void {
    const layout = layoutOf(view.size().w);
    app.dataset.layout = layout;
    header.hidden = layout === "select";
    header.classList.toggle("ow-header--strip", layout === "rail");
    panelBtn.hidden = layout !== "drawer";
  }

  const unsub = store.subscribe(render);
  const unsubView = view.subscribe(onResize);
  const tick = setInterval(render, TICK_MS);
  onResize();
  render();

  return {
    roomLabels: () => roomLabelsOn,
    setScale: (n) => setText(scale, `${n}×`),
    setDrawerOpen(open) {
      panelBtn.setAttribute("aria-expanded", String(open));
      panelBtn.setAttribute("aria-label", open ? H.panelClose : H.panelOpen);
    },
    destroy() {
      unsub();
      unsubView();
      clearInterval(tick);
      header.replaceChildren();
    },
  };
}
