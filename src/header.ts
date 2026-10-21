// 가운데 머리말 (HANDOFF 4.3)과 창 너비 구간 (PLAN 3.2). store 가 바뀌면 바뀐 칸만 고친다.
import "../design/handoff/tokens.css";
import phrases from "../design/handoff/phrases.ko.json";
import nightSvg from "../design/handoff/icons/night.svg?raw";
import panelSvg from "../design/handoff/icons/panel.svg?raw";
import roomLabelsSvg from "../design/handoff/icons/room-labels.svg?raw";
import soundOnSvg from "../design/handoff/icons/sound-on.svg?raw";
import soundOffSvg from "../design/handoff/icons/sound-off.svg?raw";
import { BUILDING_PRODUCT, DOT_STATE, FLOOR_PRODUCT, ringOf, WORK_TYPES } from "./sidebar-left.ts";
import { durationText, fill, hhmm } from "./text.ts";
import { windowViewport, type Viewport } from "./sidebar-right.ts";
import type { OfficeDto, OfficeListDto, TeamSummaryDto } from "./types/ipc.ts";

const H = phrases.header;
const TICK_MS = 30_000;

type Layout = "wide" | "drawer" | "rail" | "select";
/** 3.2 표의 너비 구간. 사이드바(900/720)와 현황판(900) 경계가 같다 */
export function layoutOf(width: number): Layout {
  return width >= 1100 ? "wide" : width >= 900 ? "drawer" : width >= 720 ? "rail" : "select";
}

// "근무 중 · {elapsed}째" 의 elapsed. 1분 미만도 1분으로 센다
function elapsedText(startedMs: number, now: number): string {
  return durationText(Math.max(60, (now - startedMs) / 1000), false);
}

export interface HeaderStore {
  state: { list: OfficeListDto | null; selectedId: string | null; office: OfficeDto | null };
  subscribe(fn: () => void): () => void;
}
export interface HeaderOptions {
  /** 창 크기 출처. 기본은 window 크기와 resize(100ms 디바운스) */
  viewport?: Viewport;
  /** 720 미만에서 panel 버튼이 옮겨 갈 사이드바 띠. 없으면 머리말에 남는다 */
  bandSlot?(): HTMLElement | null;
  togglePanel(): void;
  setRoomLabels(on: boolean): void;
  /** 사무실을 안 골랐을 때 제목 "{company} 본사" 의 이름. 없으면 기본 이름 */
  companyName?: string;
  sound?: boolean;
  onSound?: (on: boolean) => Promise<void>;
}
export interface HeaderHandle {
  roomLabels(): boolean;
  setScale(scale: number): void;
  setDrawerOpen(open: boolean): void;
  setSettings(companyName: string | null, sound: boolean): void;
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
 */
export function mountHeader(
  app: El,
  header: El,
  store: HeaderStore,
  opts: HeaderOptions,
): HeaderHandle {
  const doc = header.ownerDocument;
  header.classList.add("ow-header");
  let hq = fill(H.hq, { company: opts.companyName ?? phrases.app.brand });
  let soundOn = opts.sound ?? false;

  const square = mk(doc, "span", "ow-header__square");
  const title = mk(doc, "h1", "ow-header__title");
  const sub = mk(doc, "span", "ow-header__sub ow-header__wide");
  const slash = mk(doc, "span", "ow-header__sub ow-header__wide", "/");
  const session = mk(doc, "span", "ow-header__session ow-header__full");
  const status = mk(doc, "span", "ow-chip ow-header__wide");
  const statusDot = mk(doc, "span", "ow-dot");
  const statusNight = mk(doc, "span", "ow-icon ow-icon--sm");
  statusNight.setAttribute("aria-hidden", "true");
  statusNight.innerHTML = nightSvg;
  const statusText = mk(doc, "span", "ow-header__status");
  status.append(statusDot, statusNight, statusText);
  const work = mk(doc, "span", "ow-chip ow-header__work");
  const divider = mk(doc, "span", "ow-header__divider ow-header__wide");
  const labels = iconBtn(doc, roomLabelsSvg, H.roomLabels);
  labels.classList.add("ow-header__wide");
  labels.setAttribute("aria-pressed", "true");
  const sound = iconBtn(doc, soundOn ? soundOnSvg : soundOffSvg, soundOn ? H.soundOff : H.soundOn);
  sound.setAttribute("aria-pressed", String(soundOn));
  const soundError = mk(doc, "p", "ow-selection-error");
  soundError.setAttribute("role", "alert");
  soundError.hidden = true;
  header.after(soundError);
  sound.addEventListener("click", async () => {
    sound.disabled = true;
    soundError.hidden = true;
    try {
      await opts.onSound?.(!soundOn);
    } catch (e) {
      soundError.textContent = String(e);
      soundError.hidden = false;
    } finally {
      sound.disabled = false;
    }
  });
  const scale = mk(doc, "span", "ow-chip ow-header__scale ow-header__full", "1×");
  scale.title = H.scale;
  const panelBtn = iconBtn(doc, panelSvg, H.panelOpen);
  panelBtn.classList.add("ow-iconbtn--solid", "ow-header__panel");
  panelBtn.setAttribute("aria-expanded", "false");
  const spacer = mk(doc, "span", "ow-header__spacer");
  header.append(
    square, title, sub, slash, session, status, work, spacer,
    divider, labels, sound, scale, panelBtn,
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
      const [name, key] = BUILDING_PRODUCT[sel.building ?? ""] ?? ["", "codex"];
      accent = `var(--ow-${key})`;
      const host = sel.teams[0]?.hostBadge;
      // ponytail: 시안은 층·배포판 이름도 붙지만 DTO 에 없다, 백엔드가 주면 붙인다
      subText = host ? `${name} · ${host}` : name;
    } else if (sel) {
      accent = `var(--ow-${FLOOR_PRODUCT[(sel.floor ?? 1) - 1] ?? "codex"})`;
      const people = live.reduce((s, t) => s + 1 + t.others, 0);
      subText = fill(phrases.sidebar.floorMeta, { teams: live.length, people });
    }
    square.style.setProperty("--ow-square", accent);
    setText(title, sel?.label ?? hq);
    setText(sub, subText);
    sub.hidden = !subText;
    const sessionTitle = large ? (office?.teams[0]?.title ?? sel?.teams[0]?.title ?? "") : "";
    setText(session, sessionTitle);
    slash.hidden = session.hidden = !sessionTitle;

    const t = Date.now();
    const night = !!office?.shared.night;
    const off = !sel || sel.phase === "Off" || live.length === 0;
    statusDot.hidden = night;
    statusNight.hidden = !night;
    statusDot.dataset.state = off || !sel ? "off" : DOT_STATE[sel.dot];
    const ring = sel ? ringOf(sel.border, sel.delayed) : "";
    if (ring) statusDot.dataset.ring = ring;
    else delete statusDot.dataset.ring;
    let statusMsg = "";
    if (sel && office) {
      const started = Math.min(...live.map((x) => Date.parse(x.started)));
      statusMsg = off
        ? fill(H.allOff, { time: hhmm(t) })
        : night
          ? fill(H.night, { time: hhmm(t) })
          : Number.isFinite(started)
            ? fill(H.working, { elapsed: elapsedText(started, t) })
            : "";
    }
    status.hidden = !statusMsg;
    setText(statusText, statusMsg);

    const wt = live[0]?.workType;
    work.hidden = !wt;
    if (wt)
      setText(work, fill(H.workType, { type: phrases.workType[WORK_TYPES.indexOf(wt)] ?? "" }));
  }

  const view = opts.viewport ?? windowViewport(doc.defaultView);
  function onResize(): void {
    const layout = layoutOf(view.size().w);
    app.dataset.layout = layout;
    header.hidden = layout === "select";
    header.classList.toggle("ow-header--strip", layout === "rail");
    panelBtn.hidden = layout === "wide";
    const home = (layout === "select" && opts.bandSlot?.()) || header;
    if (panelBtn.parentElement !== home) home.append(panelBtn);
  }

  store.subscribe(render);
  view.subscribe(onResize);
  setInterval(render, TICK_MS);
  onResize();
  render();

  return {
    setSettings(companyName, on) {
      hq = fill(H.hq, { company: companyName || phrases.app.brand });
      soundOn = on;
      sound.setAttribute("aria-pressed", String(on));
      sound.setAttribute("aria-label", on ? H.soundOff : H.soundOn);
      sound.firstElementChild!.innerHTML = on ? soundOnSvg : soundOffSvg;
      render();
    },
    roomLabels: () => roomLabelsOn,
    setScale: (n) => setText(scale, `${n}×`),
    setDrawerOpen(open) {
      panelBtn.setAttribute("aria-expanded", String(open));
      panelBtn.setAttribute("aria-label", open ? H.panelClose : H.panelOpen);
    },
  };
}
