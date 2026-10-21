// 왼쪽 사이드바 (PLAN 3.3, 4.9). 목록 전체 교체를 받아 DOM 을 키(office id)로 재사용하며 고친다.
import "../design/handoff/tokens.css";
import "./styles/sidebar.css";
import phrases from "../design/handoff/phrases.ko.json";
import { durationText, fill } from "./text.ts";
import approvalSvg from "../design/handoff/icons/approval.svg?raw";
import chevronSvg from "../design/handoff/icons/chevron-down.svg?raw";
import evictedSvg from "../design/handoff/icons/evicted.svg?raw";
import infoSvg from "../design/handoff/icons/info.svg?raw";
import emptySvg from "../design/handoff/icons/empty-building.svg?raw";
import logoUrl from "../design/assets/logo.png";
import settingsSvg from "../design/handoff/icons/settings.svg?raw";
import refreshSvg from "../design/handoff/icons/refresh.svg?raw";
import type {
  Border,
  Dot,
  OfficeListDto,
  OfficeSummaryDto,
  TeamPhase,
  TeamSummaryDto,
  WorkType,
} from "./types/ipc.ts";

const P = phrases.sidebar;
const T = phrases.toast;

export interface SidebarStore {
  state: {
    list: OfficeListDto | null;
    selectedId: string | null;
    selectionError?: "recovering" | "restored" | "failed" | null;
  };
  subscribe(fn: () => void): () => void;
  selectOffice(officeId: string): number;
}
/** 백엔드에 select_office 를 보낸다. requestId 는 store.selectOffice 가 돌려준 값 */
export type SelectOffice = (officeId: string, requestId: number) => void;
export interface SidebarOptions {
  /** 창 너비. 기본은 문서 루트의 clientWidth */
  getWidth?: () => number;
  /** 행의 상세 버튼 (세션 상세를 연다). opener 는 닫을 때 초점이 돌아갈 버튼 */
  onDetail?: (officeId: string, teamId: string, name: string, opener: HTMLElement) => void;
  onTeam?: (officeId: string, teamId: string) => void;
  /** 간판의 회사 이름. 없으면 기본 이름 */
  companyName?: string;
  rawDiagnostics?: boolean;
  onSettings?: (opener: HTMLElement) => void;
  onRescan?: () => Promise<void>;
}

type SidebarMode = "full" | "rail" | "select";
const RAIL_BELOW = 900; // 3.2: 900 미만 레일
const HIDE_BELOW = 720; // 720 미만 숨김, 상단 띠 <select>
const TOAST_MS = 3000;
const ELAPSED_TICK_MS = 60_000;
const RESIZE_DEBOUNCE_MS = 100;

export const WORK_TYPES: WorkType[] = [
  "CodeReview",
  "BugFix",
  "Refactor",
  "Docs",
  "Research",
  "NewDev",
];
export const DOT_STATE: Record<Dot, string> = {
  Incident: "incident",
  Working: "working",
  Waiting: "waiting",
  Dozing: "sleepy",
  IdleAlive: "idle-alive",
  Off: "off",
};
// 상태 점은 색만으로 말하지 않는다 (HANDOFF 10절)
const DOT_WORD: Record<string, string> = {
  incident: P.stateIncident,
  working: P.stateWorking,
  waiting: P.stateWaiting,
  sleepy: P.stateSleepy,
  "idle-alive": P.idleAlive,
  off: P.stateOff,
};
// 동 글자 = 제품 (PLAN 2부). 제품이 색을 정한다
export const BUILDING_PRODUCT: Record<string, [string, string]> = {
  A: ["Claude Desktop", "claude"],
  B: ["Claude Code CLI", "claude"],
  C: ["Codex 앱", "codex"],
  D: ["Codex CLI", "codex"],
  E: ["Antigravity 앱", "gemini"],
  F: ["Antigravity CLI", "gemini"],
};
export const FLOOR_PRODUCT = ["claude", "codex", "gemini"];
const FLOOR_NAME = ["Claude", "Codex", "Gemini"];
const color = (product: string) => `var(--ow-${product})`;

// ---------- 문구 ----------

function elapsedText(startedIso: string, now: number): string {
  const sec = Math.floor((now - Date.parse(startedIso)) / 1000);
  const u = phrases.units;
  if (!(sec >= 60)) return u.now; // 시계가 어긋나 음수이거나 날짜를 못 읽어도 "방금"
  return fill(u.elapsed, { value: durationText(sec, false) });
}

// ---------- DOM 도구 ----------

type El = HTMLElement;
const mk = (doc: Document, tag: string, cls = "", text?: string): El => {
  const e = doc.createElement(tag);
  if (cls) e.className = cls;
  if (text !== undefined) e.textContent = text;
  return e;
};
const svgSpan = (doc: Document, svg: string, cls: string): El => {
  const s = mk(doc, "span", cls);
  s.innerHTML = svg; // 앱에 묶인 정적 아이콘이다
  return s;
};
/** 빈 상태 안내(아이콘 + 제목 + 본문). 사이드바 빈 목록과 캔버스 실패 화면이 같이 쓴다 */
export const emptyState = (doc: Document, title: string, body: string): El => {
  const el = mk(doc, "div", "ow-empty");
  el.append(
    svgSpan(doc, emptySvg, "ow-empty__art"),
    mk(doc, "b", "", title),
    mk(doc, "span", "", body),
  );
  return el;
};
const changed = (last: Record<string, unknown>, key: string, v: unknown): boolean =>
  last[key] === v ? false : ((last[key] = v), true);

/** parent 의 자식을 nodes 순서로 맞춘다. 이미 제자리인 노드는 건드리지 않는다 */
function place(parent: Node, nodes: Node[]): void {
  let ref: Node | null = parent.firstChild;
  for (const n of nodes) {
    if (n === ref) ref = ref.nextSibling;
    else parent.insertBefore(n, ref);
  }
  while (ref) {
    const next: Node | null = ref.nextSibling;
    parent.removeChild(ref);
    ref = next;
  }
}

// ---------- 목록 계획 ----------

interface RowModel {
  t: "row";
  key: string;
  kind: "office" | "team" | "floor";
  /** 눌렀을 때 고르는 사무실. 공실이면 null */
  targetId: string | null;
  name: string;
  workType: string;
  folder: string;
  host: string;
  metaHead: string;
  started: string | null;
  state: string;
  ring: string;
  asking: boolean;
  off: boolean;
  vacant: boolean;
  accent: string;
  hidden: boolean;
  /** 층 행만: 접기 키 */
  foldKey: string | null;
  open: boolean;
  /** 세션 상세를 열 팀. 층 행과 팀이 없는 행은 null */
  detail: { officeId: string; teamId: string } | null;
}
interface GroupModel {
  t: "group";
  key: string;
  title: string;
  sub: string;
  count: string;
  square: string | null;
  open: boolean;
  defOpen: boolean;
  hidden: boolean;
  nested: boolean;
  divider: boolean;
}
interface LabelModel {
  t: "label";
  key: "label";
  text: string;
  count: string;
}
interface EmptyModel {
  t: "empty";
  key: "empty";
}
type Item = RowModel | GroupModel | LabelModel | EmptyModel;

interface Selectable {
  id: string | null;
  label: string;
  letter: string;
  square: string;
  state: string;
  asking: boolean;
  off: boolean;
  disabled: boolean;
}
interface Plan {
  items: Item[];
  selectables: Selectable[];
}
type IsOpen = (key: string, def: boolean) => boolean;

export const ringOf = (border: Border, delayed: boolean): string =>
  // ponytail: data-ring 이 하나뿐이라 빌드 테두리가 통신 지연보다 앞선다, 둘을 같이 보일 디자인이 생기면 올린다
  border === "Failed" ? "fail" : border === "Running" ? "run" : delayed ? "lag" : "";

const workLabel = (w: WorkType | undefined): string =>
  w ? (phrases.workType[WORK_TYPES.indexOf(w)] ?? "") : "";

function metaHead(t: TeamSummaryDto, large: boolean): string {
  const tpl = large
    ? t.others === 0
      ? P.officeMetaAlone
      : P.officeMeta
    : t.others === 0
      ? P.teamMetaAlone
      : P.teamMeta;
  // 경과 칸은 따로 고치므로 앞부분만 만든다
  return fill(tpl.replace("{elapsed}째", ""), { folder: t.folder, n: t.others });
}

/** 공실이거나 사무실 id 가 없으면 고를 수 없다. 행·레일·option 이 같은 판정을 쓴다 */
const unpickable = (vacant: boolean, id: string | null): boolean => vacant || id === null;

const teamName = (t: TeamSummaryDto) => fill(P.teamName, { boss: t.bossName });

/** 팝업 제목·사무실 이름. 클릭이 준 officeId 로 목록에서 찾는다: 사무실 전환 중엔 선택과 다를 수 있다. 팀을 못 찾으면 사무실 이름 */
export function popupNames(
  list: OfficeListDto | null,
  officeId: string,
  teamId: string,
): { officeName: string; teamLabel: string } {
  const o = list?.offices.find((x) => x.officeId === officeId);
  const officeName = o?.label ?? "";
  const t = o?.teams.find((x) => x.teamId === teamId);
  return { officeName, teamLabel: t ? teamName(t) : officeName };
}
const ariaOf = (name: string, state: string, asking: boolean) =>
  [name, DOT_WORD[state], asking ? P.asking : ""].filter(Boolean).join(", ");

function group(key: string, title: string, over: Partial<GroupModel>): GroupModel {
  return {
    t: "group",
    key,
    title,
    sub: "",
    count: "",
    square: null,
    open: true,
    defOpen: true,
    hidden: false,
    nested: false,
    divider: false,
    ...over,
  };
}

function planLarge(offices: OfficeSummaryDto[], isOpen: IsOpen): Plan {
  const active = offices.filter((o) => o.phase !== "Off");
  const off = offices.filter((o) => o.phase === "Off");
  const items: Item[] = [
    { t: "label", key: "label", text: P.sectionLit, count: String(active.length) },
  ];
  const selectables: Selectable[] = [];

  const add = (o: OfficeSummaryDto, hidden: boolean): void => {
    const team = o.teams[0];
    const isOff = o.phase === "Off";
    const state = isOff ? "off" : DOT_STATE[o.dot];
    const asking = o.asking && !isOff;
    const accent = color((BUILDING_PRODUCT[o.building ?? ""] ?? ["", "codex"])[1]);
    const workType = workLabel(team?.workType);
    items.push({
      t: "row",
      key: `office:${o.officeId}`,
      kind: "office",
      targetId: o.officeId,
      name: o.label,
      workType,
      folder: team?.folder ?? "",
      host: team?.hostBadge ?? "",
      metaHead: team ? (isOff ? team.folder : metaHead(team, true)) : "",
      started: team && !isOff ? team.started : null,
      state,
      ring: ringOf(o.border, o.delayed),
      asking,
      off: isOff,
      vacant: false,
      accent,
      hidden,
      foldKey: null,
      open: true,
      detail: team ? { officeId: o.officeId, teamId: team.teamId } : null,
    });
    selectables.push({
      id: o.officeId,
      label: workType ? `${o.label} · ${workType}` : o.label,
      letter: o.building ?? "?",
      square: accent,
      state,
      asking,
      off: isOff,
      disabled: false,
    });
  };

  // 동은 글자 순서. 백엔드 정렬에 기대지 않는다
  for (const b of [...new Set(active.map((o) => o.building ?? ""))].sort()) {
    const members = active.filter((o) => (o.building ?? "") === b);
    const gkey = `group:${b}`;
    // 동이 없는 사무실은 묶지 않고 머리글 없이 행만 둔다
    const open = b ? isOpen(gkey, true) : true;
    const [product, prodKey] = BUILDING_PRODUCT[b] ?? ["", "codex"];
    if (b)
      items.push(
        group(gkey, `${b}동`, {
          sub: product,
          count: String(members.length),
          square: color(prodKey),
          open,
        }),
      );
    for (const o of members) add(o, !open);
  }
  if (off.length) {
    const open = isOpen("group:off", false);
    items.push(
      group("group:off", P.offGroup, {
        count: String(off.length),
        open,
        defOpen: false,
        divider: true,
      }),
    );
    for (const o of off) add(o, !open);
  }
  return { items, selectables };
}

function planSmall(offices: OfficeSummaryDto[], isOpen: IsOpen): Plan {
  const items: Item[] = [{ t: "label", key: "label", text: P.sectionHqFloors, count: "" }];
  const selectables: Selectable[] = [];
  for (const n of [1, 2, 3]) {
    const o = offices.find((x) => x.floor === n);
    const product = FLOOR_PRODUCT[n - 1];
    const live = o?.teams.filter((t) => t.phase !== "Off") ?? [];
    const gone = o?.teams.filter((t) => t.phase === "Off") ?? [];
    const vacant = live.length === 0;
    const fkey = `floor:${n}`;
    const open = isOpen(fkey, true);
    const incidents = live.filter((t) => t.dot === "Incident").length;
    const state = vacant || !o ? "off" : DOT_STATE[o.dot];
    const asking = !vacant && !!o?.asking;
    const name = o?.label ?? fill(P.floorName, { n, product: FLOOR_NAME[n - 1] });
    items.push({
      t: "row",
      key: o ? `floor:${o.officeId}` : `vacant:${n}`,
      kind: "floor",
      targetId: o?.officeId ?? null,
      name,
      workType: "",
      folder: "",
      host: "",
      metaHead: vacant
        ? P.vacant
        : fill(incidents ? P.floorMetaIncident : P.floorMeta, {
            teams: live.length,
            people: live.reduce((s, t) => s + 1 + t.others, 0),
            n: incidents,
          }),
      started: null,
      state,
      ring: o && !vacant ? ringOf(o.border, o.delayed) : "",
      asking,
      off: false,
      vacant,
      accent: color(product),
      hidden: false,
      foldKey: fkey,
      open,
      detail: null,
    });
    selectables.push({
      id: o?.officeId ?? null,
      label: name,
      letter: String(n),
      square: color(product),
      state,
      asking,
      off: vacant,
      disabled: unpickable(vacant, o?.officeId ?? null),
    });

    const teamRow = (t: TeamSummaryDto, isOff: boolean, hidden: boolean): RowModel => {
      const st = isOff ? "off" : DOT_STATE[t.dot];
      const nm = teamName(t);
      return {
        t: "row",
        key: `team:${t.teamId}`,
        kind: "team",
        targetId: o?.officeId ?? null,
        name: nm,
        workType: workLabel(t.workType),
        folder: t.folder,
        host: t.hostBadge ?? "",
        metaHead: isOff ? t.folder : metaHead(t, false),
        started: isOff ? null : t.started,
        state: st,
        ring: isOff ? "" : ringOf(t.border, t.delayed),
        asking: t.asking && !isOff,
        off: isOff,
        vacant: false,
        accent: color(product),
        hidden,
        foldKey: null,
        open: true,
        detail: o ? { officeId: o.officeId, teamId: t.teamId } : null,
      };
    };
    for (const t of live) items.push(teamRow(t, false, !open));
    if (gone.length) {
      const gkey = `offteams:${n}`;
      const gopen = isOpen(gkey, false);
      items.push(
        group(gkey, P.offTeams, {
          count: String(gone.length),
          open: gopen,
          defOpen: false,
          hidden: !open,
          nested: true,
        }),
      );
      for (const t of gone) items.push(teamRow(t, true, !open || !gopen));
    }
  }
  return { items, selectables };
}

// ---------- 토스트 ----------

type ToastKind = "moveIn" | "teamIn" | "back" | "off" | "evicted";
interface ToastMsg {
  kind: ToastKind;
  name: string;
}
interface Entity {
  name: string;
  phase: TeamPhase;
}

function entitiesOf(list: OfficeListDto): Map<string, Entity> {
  const m = new Map<string, Entity>();
  if (list.scale === "Large")
    for (const o of list.offices) m.set(o.officeId, { name: o.label, phase: o.phase });
  else
    for (const o of list.offices)
      for (const t of o.teams) m.set(t.teamId, { name: teamName(t), phase: t.phase });
  return m;
}

// 대기업은 사무실, 중소기업은 팀이 입주·퇴근하는 단위다 (3.3)
function diffToasts(prev: OfficeListDto, next: OfficeListDto): ToastMsg[] {
  // 규모가 바뀌었거나 조용한 재구성이면 알릴 일이 아니다 (4.5)
  if (prev.scale !== next.scale || next.rebuilt) return [];
  const a = entitiesOf(prev);
  const b = entitiesOf(next);
  const out: ToastMsg[] = [];
  for (const [id, e] of b) {
    const p = a.get(id);
    if (!p) {
      if (e.phase !== "Off")
        out.push({ kind: next.scale === "Large" ? "moveIn" : "teamIn", name: e.name });
    } else if (p.phase !== "Off" && e.phase === "Off") out.push({ kind: "off", name: e.name });
    else if (p.phase === "Off" && e.phase !== "Off") out.push({ kind: "back", name: e.name });
  }
  for (const [id, p] of a) if (!b.has(id)) out.push({ kind: "evicted", name: p.name });
  return out;
}

function toastEl(doc: Document, m: ToastMsg): El {
  const el = mk(doc, "div", "ow-toast");
  if (m.kind === "evicted") el.append(svgSpan(doc, evictedSvg, "ow-icon ow-icon--sm"));
  else {
    const dot = mk(doc, "span", "ow-dot");
    dot.dataset.state = m.kind === "off" ? "off" : "working";
    el.append(dot);
  }
  const [before, after = ""] = T[m.kind].split(/\{(?:office|team)\}/);
  const text = mk(doc, "span", "ow-toast__text");
  text.append(mk(doc, "span", "", before), mk(doc, "b", "", m.name), mk(doc, "span", "", after));
  el.append(text, mk(doc, "span", "ow-toast__ago", phrases.units.now));
  return el;
}

// ---------- 뷰 ----------

interface View {
  el: El;
  last: Record<string, unknown>;
}
interface RowView extends View {
  model: RowModel;
  main: El;
  dot: El;
  ask: El;
  name: El;
  idle: El;
  chip: El;
  host: El;
  head: El;
  elapsed: El;
  square: El | null;
  fold: El | null;
  info: El;
}
interface GroupView extends View {
  model: GroupModel;
  btn: El;
  square: El;
  title: El;
  sub: El;
  count: El;
}
interface RailView extends View {
  id: string | null;
  letter: El;
  state: El;
}

export function mountSidebar(
  root: HTMLElement,
  store: SidebarStore,
  selectOffice: SelectOffice,
  opts: SidebarOptions = {},
): (() => void) & { setSettings(companyName: string | null, rawDiagnostics: boolean): void } {
  const doc = root.ownerDocument;
  root.classList.add("ow-side-host");

  const aside = mk(doc, "aside", "ow-sidebar");
  aside.setAttribute("aria-label", phrases.header.officeSelect);
  const brand = mk(doc, "button", "ow-brand");
  brand.setAttribute("type", "button");
  brand.setAttribute("aria-label", P.openSettings);
  brand.addEventListener("click", () => opts.onSettings?.(brand));
  const logo = doc.createElement("img");
  logo.className = "ow-brand__logo ow-px";
  logo.src = logoUrl;
  logo.alt = "";
  const brandSub = mk(doc, "span", "ow-brand__sub", phrases.app.brandSub);
  const brandText = mk(doc, "span", "ow-brand__text");
  const brandName = mk(doc, "b", "ow-brand__name", opts.companyName ?? phrases.app.brand);
  brandText.append(brandName, brandSub);
  const warning = mk(doc, "span", "ow-brand__warn");
  warning.hidden = !opts.rawDiagnostics;
  warning.setAttribute("role", "img");
  warning.setAttribute("aria-label", "원문 진단 켜짐");
  brand.append(logo, brandText, warning, svgSpan(doc, settingsSvg, "ow-icon"));
  const listEl = mk(doc, "div", "ow-sidebar__list");
  const footer = mk(doc, "div", "ow-sidebar__footer", phrases.sidebar.autoCheck);
  const refresh = doc.createElement("button");
  refresh.type = "button";
  refresh.className = "ow-iconbtn";
  refresh.setAttribute("aria-label", P.refresh);
  refresh.append(svgSpan(doc, refreshSvg, "ow-icon"));
  const rescanError = mk(doc, "p", "ow-sidebar__error");
  rescanError.setAttribute("role", "alert");
  rescanError.hidden = true;
  refresh.addEventListener("click", async () => {
    refresh.disabled = true;
    rescanError.hidden = true;
    try {
      await opts.onRescan?.();
    } catch (e) {
      rescanError.textContent = `다시 찾지 못했습니다: ${String(e)}`;
      rescanError.hidden = false;
    } finally {
      refresh.disabled = false;
    }
  });
  footer.append(refresh);
  aside.append(brand, listEl, rescanError, footer);
  // 레일·드롭다운 모드에선 aside 가 숨으므로 토스트 칸은 aside 밖에 둔다
  const toastSlot = mk(doc, "div", "ow-toast-slot");
  toastSlot.setAttribute("role", "status");
  toastSlot.setAttribute("aria-live", "polite");
  const rail = mk(doc, "nav", "ow-rail");
  rail.setAttribute("aria-label", phrases.header.officeSelect);
  const band = mk(doc, "div", "ow-side-band");
  const select = doc.createElement("select");
  select.className = "ow-side-select";
  select.setAttribute("aria-label", phrases.header.officeSelect);
  const bandSettings = mk(doc, "button", "ow-iconbtn");
  bandSettings.setAttribute("type", "button");
  bandSettings.setAttribute("aria-label", P.openSettings);
  const bandWarning = mk(doc, "span", "ow-brand__warn");
  bandWarning.hidden = !opts.rawDiagnostics;
  bandWarning.setAttribute("aria-hidden", "true");
  bandSettings.append(svgSpan(doc, settingsSvg, "ow-icon"), bandWarning);
  bandSettings.addEventListener("click", () => opts.onSettings?.(bandSettings));
  band.append(select, bandSettings);
  root.append(aside, toastSlot, rail, band);

  function pick(id: string | null): void {
    root.classList.remove("is-pop");
    if (!id || (id === store.state.selectedId && store.state.selectionError !== "failed")) return;
    selectOffice(id, store.selectOffice(id));
  }

  const folds = new Map<string, boolean>();
  const isOpen: IsOpen = (k, d) => folds.get(k) ?? d;
  const toggle = (k: string, d: boolean) => {
    folds.set(k, !isOpen(k, d));
    render();
  };

  let pool = new Map<string, View>();
  let rowViews: RowView[] = [];
  let railPool = new Map<string, RailView>();
  let optSig = "";
  let lastScale: string | null = null;
  let lastList: OfficeListDto | null = null;
  let slideIn = false; // 첫 목록은 그냥 깔고, 그 뒤 새 행만 미끄러져 들어온다

  // opener: 닫을 때 초점이 돌아갈 곳. 키보드(→)는 행 버튼이라야 ↑↓ 로빙이 이어진다
  function openDetail(v: RowView, opener: HTMLElement): void {
    const d = v.model.detail;
    if (d) opts.onDetail?.(d.officeId, d.teamId, v.model.name, opener);
  }

  function newRow(m: RowModel): RowView {
    const el = mk(doc, "div", `ow-row is-${m.kind}`);
    const main = mk(doc, "button", "ow-row__main");
    main.setAttribute("type", "button");
    const lead = mk(doc, "span", "ow-row__lead");
    const dot = mk(doc, "span", "ow-dot");
    const ask = svgSpan(doc, approvalSvg, "ow-row__ask ow-icon ow-icon--sm");
    ask.hidden = true;
    lead.append(dot, ask);
    const text = mk(doc, "span", "ow-row__text");
    const top = mk(doc, "span", "ow-row__top");
    const square = m.kind === "floor" ? mk(doc, "span", "ow-square") : null;
    const name = mk(doc, "span", "ow-row__name ow-truncate");
    const idle = mk(doc, "span", "ow-chip ow-row__idle", P.idleAlive);
    const chip = mk(doc, "span", "ow-worktype");
    const host = mk(doc, "span", "ow-host");
    idle.hidden = chip.hidden = host.hidden = true;
    if (square) top.append(square);
    top.append(name, idle, chip, host);
    const meta = mk(doc, "span", "ow-row__meta ow-truncate");
    const head = mk(doc, "span", "ow-row__head");
    const elapsed = mk(doc, "span", "ow-row__elapsed");
    meta.append(head, elapsed);
    text.append(top, meta);
    main.append(lead, text);
    el.append(main);
    const info = mk(doc, "button", "ow-iconbtn ow-iconbtn--sm ow-row__info");
    info.setAttribute("type", "button");
    // 행 하나가 Tab 정지점 하나다. 키보드는 → 로 연다 (HANDOFF 10절)
    info.setAttribute("tabindex", "-1");
    info.innerHTML = infoSvg; // 앱에 묶인 정적 아이콘이다
    el.append(info);
    let fold: El | null = null;
    if (m.foldKey) {
      fold = mk(doc, "button", "ow-iconbtn ow-iconbtn--sm ow-row__fold");
      fold.setAttribute("type", "button");
      fold.innerHTML = chevronSvg;
      el.append(fold);
    }
    if (slideIn) {
      el.classList.add("is-new");
      el.addEventListener("animationend", () => el.classList.remove("is-new"));
    }
    const v: RowView = {
      el,
      last: {},
      model: m,
      main,
      dot,
      ask,
      name,
      idle,
      chip,
      host,
      head,
      elapsed,
      square,
      fold,
      info,
    };
    info.addEventListener("click", () => openDetail(v, info));
    // 모델은 갱신마다 바뀌므로 눌린 순간의 것을 본다
    main.addEventListener("click", () => {
      pick(v.model.targetId);
      if (v.model.kind === "team" && v.model.detail)
        opts.onTeam?.(v.model.detail.officeId, v.model.detail.teamId);
    });
    fold?.addEventListener("click", () => toggle(v.model.foldKey!, true));
    return v;
  }

  /** 읽어 주는 이름: 이름, 상태, 허가 대기, 업무, 폴더, 경과 */
  function fillAria(v: RowView): void {
    const m = v.model;
    const aria = [
      m.name,
      DOT_WORD[m.state],
      m.asking ? P.asking : "",
      m.workType,
      m.folder,
      v.last.elapsed as string | undefined,
    ]
      .filter(Boolean)
      .join(", ");
    if (changed(v.last, "aria", aria)) v.main.setAttribute("aria-label", aria);
  }

  function fillElapsed(v: RowView): void {
    const text = v.model.started ? elapsedText(v.model.started, Date.now()) : "";
    if (changed(v.last, "elapsed", text)) {
      v.elapsed.textContent = text;
      fillAria(v);
    }
  }

  function updateRow(v: RowView, m: RowModel, selected: boolean): void {
    const L = v.last;
    v.model = m;
    if (changed(L, "name", m.name)) {
      v.name.textContent = m.name;
      v.fold?.setAttribute("aria-label", m.name);
      v.info.setAttribute("aria-label", fill(P.detail, { office: m.name }));
    }
    if (changed(L, "detail", m.detail !== null)) v.info.hidden = m.detail === null;
    if (changed(L, "work", m.workType)) {
      v.chip.textContent = m.workType;
      v.chip.hidden = !m.workType;
    }
    if (changed(L, "host", m.host)) {
      v.host.textContent = m.host;
      v.host.hidden = !m.host;
    }
    if (changed(L, "head", m.metaHead)) v.head.textContent = m.metaHead;
    fillElapsed(v);
    if (changed(L, "state", m.state)) {
      v.dot.dataset.state = m.state;
      v.idle.hidden = m.state !== "idle-alive";
    }
    if (changed(L, "ring", m.ring)) {
      if (m.ring) v.dot.dataset.ring = m.ring;
      else delete v.dot.dataset.ring;
    }
    if (changed(L, "ask", m.asking)) v.ask.hidden = !m.asking;
    if (changed(L, "off", m.off)) v.el.classList.toggle("is-off", m.off);
    if (changed(L, "vacant", m.vacant)) v.el.classList.toggle("is-vacant", m.vacant);
    const disabled = unpickable(m.vacant, m.targetId);
    if (changed(L, "dis", disabled)) {
      if (disabled) v.main.setAttribute("disabled", "");
      else v.main.removeAttribute("disabled");
    }
    if (changed(L, "hidden", m.hidden)) v.el.hidden = m.hidden;
    fillAria(v);
    if (changed(L, "accent", m.accent)) {
      v.el.style.setProperty("--ow-row-accent", m.accent);
      v.square?.style.setProperty("--ow-square", m.accent);
    }
    if (changed(L, "sel", selected)) {
      v.el.classList.toggle("is-selected", selected);
      if (selected) v.main.setAttribute("aria-current", "true");
      else v.main.removeAttribute("aria-current");
    }
    if (v.fold && changed(L, "open", m.open)) v.fold.setAttribute("aria-expanded", String(m.open));
  }

  function newGroup(m: GroupModel): GroupView {
    const el = mk(doc, "div", "ow-group-wrap");
    const btn = mk(doc, "button", "ow-group");
    btn.setAttribute("type", "button");
    const square = mk(doc, "span", "ow-square");
    const title = mk(doc, "b");
    const sub = mk(doc, "span", "ow-group__sub ow-truncate");
    const count = mk(doc, "span", "ow-count");
    btn.append(
      svgSpan(doc, chevronSvg, "ow-group__arrow ow-icon ow-icon--sm"),
      square,
      title,
      sub,
      count,
    );
    el.append(btn);
    const v: GroupView = { el, last: {}, model: m, btn, square, title, sub, count };
    btn.addEventListener("click", () => toggle(v.model.key, v.model.defOpen));
    return v;
  }

  function updateGroup(g: GroupView, m: GroupModel): void {
    const L = g.last;
    g.model = m;
    if (changed(L, "title", m.title)) g.title.textContent = m.title;
    if (changed(L, "sub", m.sub)) g.sub.textContent = m.sub;
    if (changed(L, "count", m.count)) g.count.textContent = m.count;
    if (changed(L, "open", m.open)) g.btn.setAttribute("aria-expanded", String(m.open));
    if (changed(L, "sq", m.square ?? "")) {
      g.square.hidden = !m.square;
      if (m.square) g.square.style.setProperty("--ow-square", m.square);
    }
    if (changed(L, "hidden", m.hidden)) g.el.hidden = m.hidden;
    g.el.classList.toggle("is-nested", m.nested);
    g.el.classList.toggle("has-divider", m.divider);
  }

  function viewOf(m: Item, selectedId: string | null): View {
    const old = pool.get(m.key);
    if (m.t === "label") {
      const v = old ?? { el: mk(doc, "div", "ow-section-label"), last: {} };
      if (!old) v.el.append(mk(doc, "span"), mk(doc, "span", "ow-count"));
      if (changed(v.last, "t", m.text)) v.el.firstChild!.textContent = m.text;
      if (changed(v.last, "c", m.count)) v.el.lastChild!.textContent = m.count;
      return v;
    }
    if (m.t === "empty") {
      if (old) return old;
      return { el: emptyState(doc, P.emptyTitle, P.emptyBody), last: {} };
    }
    if (m.t === "group") {
      const g = (old as GroupView | undefined) ?? newGroup(m);
      updateGroup(g, m);
      return g;
    }
    const r = (old as RowView | undefined) ?? newRow(m);
    // 선택 표시는 사무실·층 행에만 둔다. 팀 행을 눌러도 표시는 그 층 행에 간다
    updateRow(r, m, m.kind !== "team" && m.targetId !== null && m.targetId === selectedId);
    return r;
  }

  function syncList(items: Item[], selectedId: string | null): void {
    const next = new Map<string, View>();
    const nextRows: RowView[] = [];
    for (const m of items) {
      const v = viewOf(m, selectedId);
      next.set(m.key, v);
      if (m.t === "row") nextRows.push(v as RowView);
    }
    pool = next;
    rowViews = nextRows;
    place(
      listEl,
      [...next.values()].map((v) => v.el),
    );
    ensureRoving();
  }

  // ----- 로빙 tabindex: 행 중 Tab 정지점은 하나 (HANDOFF 10절) -----
  const pickableMains = (): El[] =>
    rowViews.filter((v) => !v.el.hidden && !v.main.hasAttribute("disabled")).map((v) => v.main);
  function setRoving(active: El | null): void {
    for (const v of rowViews) {
      const t = v.main === active ? "0" : "-1";
      if (v.main.getAttribute("tabindex") !== t) v.main.setAttribute("tabindex", t);
    }
  }
  function ensureRoving(): void {
    const mains = pickableMains();
    const cur = mains.find((m) => m.getAttribute("tabindex") === "0");
    setRoving(cur ?? mains.find((m) => m.hasAttribute("aria-current")) ?? mains[0] ?? null);
  }
  listEl.addEventListener("focusin", (e) => {
    const t = e.target as El;
    if (t.classList.contains("ow-row__main")) setRoving(t);
  });
  listEl.addEventListener("keydown", (e) => {
    if (e.key === "ArrowRight") {
      const row = rowViews.find((v) => v.main === e.target);
      if (row?.model.detail) {
        e.preventDefault();
        openDetail(row, row.main);
      }
      return;
    }
    if (e.key !== "ArrowDown" && e.key !== "ArrowUp") return;
    const mains = pickableMains();
    const i = mains.indexOf(e.target as El);
    if (i < 0) return;
    e.preventDefault();
    mains[i + (e.key === "ArrowDown" ? 1 : -1)]?.focus();
  });

  function syncRail(sel: Selectable[], selectedId: string | null): void {
    const next = new Map<string, RailView>();
    sel.forEach((s, i) => {
      const key = s.id ?? `vacant:${i}`;
      let v = railPool.get(key);
      if (!v) {
        const el = mk(doc, "button", "ow-rail__item");
        el.setAttribute("type", "button");
        const letter = mk(doc, "span", "ow-rail__letter");
        const state = mk(doc, "span", "ow-rail__state");
        el.append(letter, state);
        const made: RailView = { el, last: {}, id: s.id, letter, state };
        el.addEventListener("click", () => openOverlayAt(made.id));
        v = made;
      }
      v.id = s.id;
      if (changed(v.last, "l", s.letter)) v.letter.textContent = s.letter;
      if (changed(v.last, "sq", s.square)) v.letter.style.setProperty("--ow-square", s.square);
      // 허가 대기는 점 자리를 결재 색으로 칠한다 (HANDOFF 4.2)
      const st = s.asking ? "ask" : s.state;
      if (changed(v.last, "st", st)) v.state.dataset.state = st;
      const aria = ariaOf(s.label, s.state, s.asking);
      if (changed(v.last, "a", aria)) {
        v.el.setAttribute("aria-label", aria);
        v.el.title = s.label;
      }
      if (changed(v.last, "off", s.off)) v.el.classList.toggle("is-off", s.off);
      if (changed(v.last, "dis", s.disabled)) {
        if (s.disabled) v.el.setAttribute("disabled", "");
        else v.el.removeAttribute("disabled");
      }
      const cur = s.id !== null && s.id === selectedId;
      if (changed(v.last, "cur", cur)) {
        if (cur) v.el.setAttribute("aria-current", "true");
        else v.el.removeAttribute("aria-current");
      }
      next.set(key, v);
    });
    railPool = next;
    place(
      rail,
      [...next.values()].map((v) => v.el),
    );
  }

  function syncSelect(sel: Selectable[], selectedId: string | null): void {
    // 같은 목록을 다시 만들면 열려 있는 드롭다운이 닫힌다
    const sig = sel.map((s) => [s.id, s.label, s.disabled].join("|")).join("\n");
    if (sig !== optSig) {
      optSig = sig;
      select.replaceChildren(
        ...sel.map((s) => {
          const o = doc.createElement("option");
          o.value = s.id ?? "";
          o.textContent = s.label;
          o.disabled = s.disabled;
          return o;
        }),
      );
    }
    // 선택이 없을 때 value="" 는 id 없는 공실 option 과 겹친다
    select.selectedIndex = sel.findIndex((s) => s.id !== null && s.id === selectedId);
  }

  const queue: ToastMsg[] = [];
  let showing = false;
  let toastTimer: ReturnType<typeof setTimeout> | undefined;
  function pump(): void {
    if (showing) return;
    const m = queue.shift();
    if (!m) return;
    showing = true;
    const el = toastEl(doc, m);
    toastSlot.append(el);
    root.classList.add("has-toast");
    toastTimer = setTimeout(() => {
      el.remove();
      root.classList.remove("has-toast");
      showing = false;
      pump();
    }, TOAST_MS);
  }

  function render(): void {
    const list = store.state.list;
    if (!list) return;
    if (list !== lastList) {
      if (lastList) {
        queue.push(...diffToasts(lastList, list));
        pump();
      }
      // 조용한 재구성(4.5)이면 새 행도 애니메이션 없이 놓는다
      slideIn = lastList !== null && !list.rebuilt;
      if (lastScale !== null && lastScale !== list.scale) {
        // 규모가 바뀌면 구조가 다르다: 키가 겹쳐도 같은 행이 아니다
        pool = new Map();
        rowViews = [];
        railPool = new Map();
        folds.clear();
        place(listEl, []);
        slideIn = false;
      }
      lastScale = list.scale;
      lastList = list;
    }
    root.dataset.scale = list.scale === "Large" ? "large" : "small";
    brandSub.textContent =
      list.scale === "Large" ? phrases.app.brandSub : phrases.app.brandSubSmall;
    const plan: Plan =
      list.offices.length === 0
        ? { items: [{ t: "empty", key: "empty" }], selectables: [] }
        : list.scale === "Large"
          ? planLarge(list.offices, isOpen)
          : planSmall(list.offices, isOpen);
    syncList(plan.items, store.state.selectedId);
    syncRail(plan.selectables, store.state.selectedId);
    syncSelect(plan.selectables, store.state.selectedId);
  }

  // ----- 창 너비 구간 (3.2) -----
  const getWidth = opts.getWidth ?? (() => doc.documentElement.clientWidth);
  function applyMode(): void {
    const w = getWidth();
    const mode: SidebarMode = w < HIDE_BELOW ? "select" : w < RAIL_BELOW ? "rail" : "full";
    if (root.dataset.mode === mode) return;
    root.dataset.mode = mode;
    root.classList.remove("is-pop");
  }
  applyMode();
  let resizeTimer: ReturnType<typeof setTimeout> | undefined;
  const ro =
    typeof ResizeObserver === "undefined"
      ? null
      : new ResizeObserver(() => {
          clearTimeout(resizeTimer);
          resizeTimer = setTimeout(applyMode, RESIZE_DEBOUNCE_MS);
        });
  ro?.observe(doc.documentElement);

  // 레일에 올리거나 초점이 가면 240 이 겹쳐 열린다 (HANDOFF 2.2)
  const pop = () => {
    if (root.dataset.mode === "rail") root.classList.add("is-pop");
  };
  // 레일 항목을 누르면 고르지 않고 오버레이를 열어 그 사무실 행으로 초점을 옮긴다 (선택은 행에서)
  function openOverlayAt(id: string | null): void {
    pop();
    const row = rowViews.find(
      (v) => v.model.kind !== "team" && v.model.targetId === id && !v.el.hidden,
    );
    row?.main.focus();
  }
  const onLeave = () => root.classList.remove("is-pop");
  const onFocusOut = (e: FocusEvent) => {
    const next = e.relatedTarget as Node | null;
    if (!next || !root.contains(next)) root.classList.remove("is-pop");
  };
  rail.addEventListener("mouseenter", pop);
  rail.addEventListener("focusin", pop);
  root.addEventListener("mouseleave", onLeave);
  root.addEventListener("focusout", onFocusOut);
  select.addEventListener("change", () => pick(select.value || null));

  const tick = setInterval(() => {
    for (const v of rowViews) fillElapsed(v);
  }, ELAPSED_TICK_MS);

  const unsub = store.subscribe(render);
  render();

  return Object.assign(
    () => {
      unsub();
      ro?.disconnect();
      clearInterval(tick);
      clearTimeout(toastTimer);
      clearTimeout(resizeTimer);
      queue.length = 0;
      showing = false;
      root.removeEventListener("mouseleave", onLeave);
      root.removeEventListener("focusout", onFocusOut);
      root.classList.remove("ow-side-host", "is-pop");
      delete root.dataset.mode;
      delete root.dataset.scale;
      aside.remove();
      toastSlot.remove();
      rail.remove();
      band.remove();
    },
    {
      setSettings(companyName: string | null, rawDiagnostics: boolean) {
        brandName.textContent = companyName || phrases.app.brand;
        warning.hidden = !rawDiagnostics;
        bandWarning.hidden = !rawDiagnostics;
        bandSettings.title = rawDiagnostics ? "원문 진단 켜짐" : P.openSettings;
      },
    },
  );
}
