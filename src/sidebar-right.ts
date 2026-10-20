// 오른쪽 직원 현황판 (PLAN 3.5, 4.9, HANDOFF 4.5). 카드는 agent_id 키로 재사용한다.
import "../design/handoff/tokens.css";
import "./styles/panel.css";
import phrases from "../design/handoff/phrases.ko.json";
import iconApproval from "../design/handoff/icons/approval.svg?raw";
import iconClose from "../design/handoff/icons/close.svg?raw";
import iconEdit from "../design/handoff/icons/edit.svg?raw";
import iconFile from "../design/handoff/icons/file.svg?raw";
import iconIncident from "../design/handoff/icons/incident-count.svg?raw";
import iconMcp from "../design/handoff/icons/mcp.svg?raw";
import iconTerminal from "../design/handoff/icons/terminal.svg?raw";
import iconWeb from "../design/handoff/icons/web.svg?raw";
import roomAway from "../design/handoff/icons/room-away.svg?raw";
import roomBoss from "../design/handoff/icons/room-boss.svg?raw";
import roomBuild from "../design/handoff/icons/room-build.svg?raw";
import roomDev from "../design/handoff/icons/room-dev.svg?raw";
import roomDoor from "../design/handoff/icons/room-door.svg?raw";
import roomIncident from "../design/handoff/icons/room-incident.svg?raw";
import roomLibrary from "../design/handoff/icons/room-library.svg?raw";
import roomMeeting from "../design/handoff/icons/room-meeting.svg?raw";
import roomPantry from "../design/handoff/icons/room-pantry.svg?raw";
import roomReview from "../design/handoff/icons/room-review.svg?raw";
import roomShipping from "../design/handoff/icons/room-shipping.svg?raw";
import roomTest from "../design/handoff/icons/room-test.svg?raw";
import { createEventCollector, createTicker, fill, hhmm, rankLabel, teamName } from "./ticker.ts";
import type {
  Activity,
  CueDto,
  EmployeeDto,
  OfficeDto,
  OfficeListDto,
  Role,
  Room,
  TeamDto,
} from "./types/ipc.ts";

/** 이 너비 미만이면 현황판을 접고 드로어로 (PLAN 3.2) */
const DRAWER_BELOW = 1100;
/** 이 높이 미만이면 티커를 숨기고 카드를 압축형으로 */
const COMPACT_BELOW = 640;
const FLASH_MS = 1000;
const TOAST_MS = 3000;
const NEW_MS = 300;
const HIGHLIGHT_MS = 2000;
const QUOTE_MAX = 60;
const CTX_CELLS = 5;
const CTX_DEFAULT_WINDOW = 200_000;
const TEAM_COLORS = 6;

const ROOM_ICON: Record<Room, string> = {
  TeamLead: roomBoss,
  Dev: roomDev,
  Archive: roomLibrary,
  Build: roomBuild,
  Test: roomTest,
  Review: roomReview,
  Incident: roomIncident,
  Meeting: roomMeeting,
  Shipping: roomShipping,
  Pantry: roomPantry,
  Outside: roomAway,
  Door: roomDoor,
};

const ROOM_LABEL: Record<Room, string> = {
  TeamLead: phrases.rooms.boss,
  Dev: phrases.rooms.dev,
  Archive: phrases.rooms.library,
  Build: phrases.rooms.build,
  Test: phrases.rooms.test,
  Review: phrases.rooms.review,
  Incident: phrases.rooms.incident,
  Meeting: phrases.rooms.meeting,
  Shipping: phrases.rooms.shipping,
  Pantry: phrases.rooms.pantry,
  Outside: phrases.rooms.away,
  Door: phrases.rooms.door,
};

const ROLE_LABEL: Record<Role, string> = {
  Backend: phrases.roles.backend,
  Frontend: phrases.roles.frontend,
  Qa: phrases.roles.qa,
  DevOps: phrases.roles.devops,
  Writer: phrases.roles.writer,
  Trainee: phrases.roles.trainee,
};

const STATS = [
  { key: "read", icon: iconFile, title: phrases.card.stats.read },
  { key: "edit", icon: iconEdit, title: phrases.card.stats.edit },
  { key: "shell", icon: iconTerminal, title: phrases.card.stats.cmd },
  { key: "outside", icon: iconWeb, title: phrases.card.stats.web },
  { key: "meeting", icon: iconMcp, title: phrases.card.stats.mcp },
  { key: "errors", icon: iconIncident, title: phrases.card.stats.incident },
] as const;

export interface Viewport {
  size(): { w: number; h: number };
  subscribe(cb: () => void): () => void;
}

export interface PanelOptions {
  /** 창 크기 출처. 기본은 window 크기와 resize(100ms 디바운스) */
  viewport?: Viewport;
  /** 드로어가 열리고 닫힐 때 (머리말 버튼의 aria-expanded 용) */
  onDrawerChange?: (open: boolean) => void;
}

export interface PanelHandle {
  openDrawer(): void;
  toggleDrawer(): void;
  destroy(): void;
}

interface PanelStore {
  state: { list: OfficeListDto | null; selectedId: string | null; office: OfficeDto | null };
  subscribe(fn: () => void): () => void;
}

// ---- 작은 도우미 ----

function h<K extends keyof HTMLElementTagNameMap>(
  doc: Document,
  tag: K,
  cls = "",
  ...kids: (Node | string)[]
): HTMLElementTagNameMap[K] {
  const e = doc.createElement(tag);
  if (cls) e.className = cls;
  if (kids.length) e.append(...kids);
  return e;
}

function icon(doc: Document, svg: string): HTMLElement {
  const s = h(doc, "span", "ow-icon");
  s.setAttribute("aria-hidden", "true");
  s.innerHTML = svg;
  return s;
}

/** 백엔드 문장의 `코드` 조각을 ow-code 로 */
function setRich(node: HTMLElement, text: string): void {
  const doc = node.ownerDocument;
  node.textContent = "";
  text.split("`").forEach((seg, i) => {
    if (!seg) return;
    node.append(i % 2 ? h(doc, "span", "ow-code", seg) : seg);
  });
}

/** 시간이 흐르는 문장의 `{elapsed}` 를 채운다. 백틱 코드 조각 안은 그대로 둔다 */
function fillElapsed(sentence: string, sec: number | null): string {
  if (!sentence.includes("{elapsed}")) return sentence;
  const value = sec === null ? "" : fmtSec(sec);
  return sentence
    .split("`")
    .map((seg, i) => (i % 2 ? seg : seg.split("{elapsed}").join(value)))
    .join("`");
}

function fmtSec(sec: number): string {
  if (sec < 60) return fill(phrases.units.sec, { n: sec });
  const m = Math.floor(sec / 60);
  const s = sec % 60;
  return s
    ? `${fill(phrases.units.min, { n: m })} ${fill(phrases.units.sec, { n: s })}`
    : fill(phrases.units.min, { n: m });
}

/** 팀 경과: 60초 미만 "방금", 이후 "12분째", "1시간 4분째" (HANDOFF 8절) */
function fmtTeamElapsed(sec: number): string {
  if (sec < 60) return phrases.units.now;
  const min = Math.floor(sec / 60);
  const value =
    min < 60
      ? fill(phrases.units.min, { n: min })
      : fill(phrases.units.hourMin, { h: Math.floor(min / 60), m: min % 60 });
  return fill(phrases.units.elapsed, { value });
}

const cap99 = (n: number): string => (n > 99 ? "99+" : String(n));

function roomLabel(room: Room, small: boolean): string {
  return room === "TeamLead" && small ? phrases.rooms.bossSmall : ROOM_LABEL[room];
}

/** 부장 맨 위, 그 아래 마지막 사건이 최근인 순, 탕비실은 맨 아래 (PLAN 3.5). 부장은 team.employees[0] */
function orderEmployees(team: TeamDto): EmployeeDto[] {
  const [boss, ...rest] = team.employees;
  const idle = (e: EmployeeDto) => (e.activity.room === "Pantry" ? 1 : 0);
  const at = (e: EmployeeDto) => Date.parse(e.activity.since) || 0;
  rest.sort((a, b) => idle(a) - idle(b) || at(b) - at(a));
  return boss ? [boss, ...rest] : rest;
}

// ---- 카드 ----

interface CardEntry {
  el: HTMLButtonElement;
  last: Map<string, string>;
  timers: Map<string, ReturnType<typeof setTimeout>>;
  ref: {
    name: HTMLElement;
    rank: HTMLElement;
    reject: HTMLElement;
    sub: HTMLElement;
    model: HTMLElement;
    ctx: HTMLElement;
    ctxCells: HTMLElement[];
    ctxText: HTMLElement;
    status: HTMLElement;
    statusIcon: HTMLElement;
    statusLabel: HTMLElement;
    statusEst: HTMLElement;
    nowText: HTMLElement;
    progress: HTMLElement;
    bar: HTMLElement;
    barCaption: HTMLElement;
    barLong: HTMLElement;
    trail: { row: HTMLElement; time: HTMLElement; text: HTMLElement }[];
    stats: HTMLElement[];
    quote: HTMLElement;
  };
  sentence: string;
  sinceMs: number;
  /** 진행 막대용 평균 소요(초). 없으면 막대를 숨긴다 */
  expected: number;
}

function createCard(doc: Document, agentId: string): CardEntry {
  const el = h(doc, "button", "ow-card");
  el.type = "button";
  el.dataset.agentId = agentId;

  // ponytail: 아바타는 단색 자리표시, 캔버스 티켓에서 올빼미 스프라이트로 바꾼다
  const avatar = h(doc, "span", "ow-avatar");
  avatar.setAttribute("aria-hidden", "true");
  const name = h(doc, "span", "ow-card__name");
  const rank = h(doc, "span", "ow-card__rank");
  const reject = icon(doc, iconApproval);
  reject.classList.add("ow-card__reject");
  reject.hidden = true;
  // icon() 이 aria-hidden 을 달지만 반려 아이콘은 뜻을 전하므로 이름을 준다
  reject.removeAttribute("aria-hidden");
  reject.setAttribute("role", "img");
  reject.setAttribute("aria-label", phrases.sentence.rejected.split(":")[0]);
  const sub = h(doc, "span", "ow-card__sub");
  const id = h(
    doc,
    "span",
    "ow-card__id",
    h(doc, "span", "ow-card__line", name, rank, reject),
    sub,
  );

  const model = h(doc, "span", "ow-model");
  const ctxCells = Array.from({ length: CTX_CELLS }, () => h(doc, "i"));
  const ctxText = h(doc, "b");
  const ctx = h(doc, "span", "ow-ctx", ...ctxCells, ctxText);
  ctx.setAttribute("role", "img");
  const meta = h(doc, "span", "ow-card__meta", model, ctx);

  const statusIcon = h(doc, "span", "ow-status__icon");
  const statusLabel = h(doc, "span", "ow-status__label");
  const statusEst = h(doc, "small", "", phrases.card.estimate);
  const status = h(doc, "span", "ow-status", statusIcon, statusLabel, statusEst);

  const nowText = h(doc, "span", "ow-now__text");
  const now = h(doc, "span", "ow-now", nowText);

  const bar = h(doc, "span", "ow-progress__bar");
  const barCaption = h(doc, "span");
  const barLong = h(doc, "span", "ow-progress__long", phrases.card.tooLong);
  const progress = h(
    doc,
    "span",
    "ow-progress",
    h(doc, "span", "ow-progress__track", bar),
    h(doc, "span", "ow-progress__caption", barCaption, barLong),
  );

  const trail = Array.from({ length: 3 }, () => {
    const time = h(doc, "span", "ow-trail__time");
    const text = h(doc, "span", "ow-trail__text");
    return { row: h(doc, "span", "ow-trail__row", time, text), time, text };
  });

  const stats = STATS.map((s) => {
    const n = h(doc, "span", "ow-stats__n");
    const cell = h(doc, "span", "", icon(doc, s.icon), n);
    cell.title = s.title;
    cell.setAttribute("role", "img");
    return cell;
  });
  const quote = h(doc, "span", "ow-quote");

  el.append(
    h(doc, "span", "ow-card__head", avatar, id, meta),
    status,
    now,
    progress,
    h(doc, "span", "ow-trail", ...trail.map((t) => t.row)),
    h(doc, "span", "ow-stats", ...stats),
    quote,
  );

  return {
    el,
    last: new Map(),
    timers: new Map(),
    ref: {
      name,
      rank,
      reject,
      sub,
      model,
      ctx,
      ctxCells,
      ctxText,
      status,
      statusIcon,
      statusLabel,
      statusEst,
      nowText,
      progress,
      bar,
      barCaption,
      barLong,
      trail,
      stats,
      quote,
    },
    sentence: "",
    sinceMs: NaN,
    expected: 0,
  };
}

function changed(c: { last: Map<string, string> }, key: string, value: string): boolean {
  if (c.last.get(key) === value) return false;
  c.last.set(key, value);
  return true;
}

function setText(c: CardEntry, key: string, node: HTMLElement, value: string): void {
  if (changed(c, key, value)) node.textContent = value;
}

function setRichCached(c: CardEntry, key: string, node: HTMLElement, value: string): void {
  if (changed(c, key, value)) setRich(node, value);
}

function updateCard(
  c: CardEntry,
  e: EmployeeDto,
  isBoss: boolean,
  off: boolean,
  small: boolean,
  byId: Map<string, EmployeeDto>,
): void {
  const r = c.ref;
  const act: Activity = e.activity;
  c.el.classList.toggle("is-boss", isBoss);
  c.el.classList.toggle("is-idle", act.room === "Pantry" || off);

  setText(c, "name", r.name, e.name);
  setText(c, "rank", r.rank, rankLabel(e.rank));
  const role = fill(phrases.card.role, { role: ROLE_LABEL[e.role] });
  const parent = e.depth >= 2 && e.parent ? byId.get(e.parent) : undefined;
  setText(
    c,
    "sub",
    r.sub,
    parent
      ? fill(phrases.card.roleWithBoss, {
          role: ROLE_LABEL[e.role],
          parent: `${parent.name} ${rankLabel(parent.rank)}`,
        })
      : role,
  );

  const model = e.model ?? phrases.card.modelUnknown;
  if (changed(c, "model", model)) {
    r.model.textContent = model;
    r.model.title = model;
  }

  // 컨텍스트 눈금: 켜진 칸 = round(사용률/20). 창 크기를 모르면 20만 토큰으로 어림하고 "?" (PLAN 3.5)
  const unknownWindow = e.contextWindow === null;
  const pct =
    e.contextUsed === null
      ? null
      : Math.min(100, Math.round((e.contextUsed / (e.contextWindow || CTX_DEFAULT_WINDOW)) * 100));
  const ctxKey = pct === null ? "" : `${pct}${unknownWindow ? "?" : ""}`;
  if (changed(c, "ctx", ctxKey)) {
    r.ctx.hidden = pct === null;
    const lit = pct === null ? 0 : Math.round(pct / 20);
    r.ctxCells.forEach((cell, i) => cell.classList.toggle("is-on", i < lit));
    r.ctx.classList.toggle("is-high", pct !== null && pct >= 80);
    r.ctxText.textContent = ctxKey ? `${pct}%${unknownWindow ? "?" : ""}` : "";
    const label =
      pct === null
        ? ""
        : fill(unknownWindow ? phrases.card.contextUnknown : phrases.card.context, { pct });
    r.ctx.setAttribute("aria-label", label);
    r.ctx.title = label;
  }

  // 상태 칩: 추정이면 점선 + "추정", 허가 대기(출입문)는 결재판
  const estimate = act.confidence === "Inferred";
  const ask = act.room === "Door";
  const roomKey = `${act.room}|${small}`;
  if (changed(c, "room", roomKey)) {
    r.statusIcon.innerHTML = "";
    r.statusIcon.append(icon(c.el.ownerDocument, ask ? iconApproval : ROOM_ICON[act.room]));
    r.statusLabel.textContent = roomLabel(act.room, small);
    r.status.classList.toggle("is-bad", act.room === "Incident");
    r.status.classList.toggle("is-ask", ask);
  }
  if (changed(c, "est", String(estimate))) {
    r.status.classList.toggle("is-estimate", estimate);
    r.statusEst.hidden = !estimate;
  }

  c.sentence = act.sentence;
  c.sinceMs = Date.parse(act.since);
  const expected =
    act.expectedSecs !== null && act.expectedSecs > 0 ? Math.round(act.expectedSecs) : 0;
  c.expected = Number.isNaN(c.sinceMs) ? 0 : expected;

  // 끝난 일이 곧바로 recent 맨 앞에 들어오므로 지금 활동과 같은 줄은 뺀다. 최신이 먼저
  const trail = e.recent.filter((a) => a.sentence !== act.sentence || a.since !== act.since);
  r.trail.forEach((t, i) => {
    const a = trail[i];
    const key = a ? `${a.since}|${a.sentence}` : "";
    if (!changed(c, `trail${i}`, key)) return;
    t.row.hidden = !a;
    if (!a) return;
    const at = Date.parse(a.since);
    t.time.textContent = Number.isNaN(at) ? "" : hhmm(at);
    setRich(t.text, a.sentence);
  });

  STATS.forEach((s, i) => {
    const value = cap99(e.counters[s.key]);
    if (!changed(c, `stat${i}`, value)) return;
    (r.stats[i].lastElementChild as HTMLElement).textContent = value;
    r.stats[i].setAttribute("aria-label", `${s.title} ${value}`);
  });

  const say = e.lastSay === null ? "" : e.lastSay;
  if (changed(c, "say", say)) {
    r.quote.hidden = say === "";
    const chars = Array.from(say);
    const cut = chars.length > QUOTE_MAX ? `${chars.slice(0, QUOTE_MAX).join("")}…` : say;
    r.quote.textContent = say ? `“${cut}”` : "";
  }
}

/** 매초: 경과가 든 문장과 진행 막대만 고친다 */
function tickCard(c: CardEntry, nowMs: number): void {
  const r = c.ref;
  const sec = Number.isNaN(c.sinceMs) ? null : Math.max(0, Math.floor((nowMs - c.sinceMs) / 1000));
  setRichCached(c, "now", r.nowText, fillElapsed(c.sentence, sec));
  if (c.expected === 0 || sec === null) {
    if (changed(c, "prog", "off")) r.progress.hidden = true;
    return;
  }
  if (changed(c, "prog", "on")) r.progress.hidden = false;
  const over = sec > c.expected;
  const width = `${Math.min(100, Math.round((sec / c.expected) * 100))}%`;
  if (changed(c, "bar", `${width}|${over}`)) {
    r.bar.style.width = width;
    r.progress.classList.toggle("is-over", over);
    r.barLong.hidden = !over;
  }
  // 캡션은 막대 비율과 따로 매초 바뀐다: 평균이 길거나 넘겼어도 멈추지 않게
  if (changed(c, "cap", `${sec}|${c.expected}`))
    r.barCaption.textContent = fill(phrases.card.progress, { sec, avg: c.expected });
}

// ---- 팀 머리글 (중소기업) ----

interface HeadEntry {
  el: HTMLElement;
  last: Map<string, string>;
  square: HTMLElement;
  name: HTMLElement;
  meta: HTMLElement;
  host: HTMLElement;
  dot: HTMLElement;
  startedMs: number;
  folder: string;
}

function createHead(doc: Document): HeadEntry {
  const square = h(doc, "span", "ow-square");
  const name = h(doc, "b");
  const meta = h(doc, "span", "ow-teamhead__meta");
  const host = h(doc, "span", "ow-host");
  const dot = h(doc, "span", "ow-dot");
  dot.style.marginLeft = "auto";
  const el = h(doc, "div", "ow-teamhead", square, name, meta, host, dot);
  return { el, last: new Map(), square, name, meta, host, dot, startedMs: NaN, folder: "" };
}

function updateHead(hd: HeadEntry, t: TeamDto, color: number): void {
  const title = teamName(t);
  if (changed(hd, "name", title)) hd.name.textContent = title;
  if (changed(hd, "color", String(color)))
    hd.square.style.setProperty("--ow-square", `var(--ow-team-${color})`);
  if (changed(hd, "host", t.hostBadge ?? "")) {
    hd.host.textContent = t.hostBadge ?? "";
    hd.host.hidden = t.hostBadge === null;
  }
  const state = t.incident
    ? "incident"
    : t.phase === "Working"
      ? "working"
      : t.phase === "Dozing"
        ? "sleepy"
        : "off";
  if (changed(hd, "dot", state)) hd.dot.setAttribute("data-state", state);
  hd.startedMs = Date.parse(t.started);
  hd.folder = t.folder;
}

function tickHead(hd: HeadEntry, nowMs: number): void {
  const elapsed = Number.isNaN(hd.startedMs)
    ? ""
    : fmtTeamElapsed(Math.max(0, Math.floor((nowMs - hd.startedMs) / 1000)));
  const text = elapsed ? `${hd.folder} · ${elapsed}` : hd.folder;
  if (changed(hd, "meta", text)) hd.meta.textContent = text;
}

// ---- 현황판 ----

export function mountPanel(
  root: HTMLElement,
  store: PanelStore,
  opts: PanelOptions = {},
): PanelHandle {
  const doc = root.ownerDocument;

  const title = h(doc, "h2", "ow-panel__title", phrases.card.panelTitle);
  const count = h(doc, "span", "ow-chip ow-panel__count");
  const officeName = h(doc, "span", "ow-panel__office");
  const closeBtn = h(doc, "button", "ow-iconbtn ow-panel__close");
  closeBtn.type = "button";
  closeBtn.setAttribute("aria-label", phrases.header.panelClose);
  closeBtn.append(icon(doc, iconClose));
  const head = h(doc, "div", "ow-panel__head", title, count, officeName, closeBtn);

  const toasts = h(doc, "div", "ow-panel__toasts");
  toasts.setAttribute("role", "status");
  toasts.setAttribute("aria-live", "polite");
  const empty = h(doc, "p", "ow-panel__empty", phrases.card.empty);
  const list = h(doc, "div", "ow-panel__list");
  const ticker = createTicker(doc, pick);
  const aside = h(doc, "aside", "ow-panel", head, toasts, empty, list, ticker.el);
  aside.setAttribute("aria-label", phrases.card.panelTitle);
  root.append(aside);

  const cards = new Map<string, CardEntry>();
  const heads = new Map<string, HeadEntry>();
  const teamColor = new Map<string, number>();
  const events = createEventCollector();
  const toastTimers = new Set<ReturnType<typeof setTimeout>>();
  let lastOfficeId: string | null = null;
  let lastCueKey = "";
  let compact = false;
  let narrow = false;
  let drawerOpen = false;

  // ---- 연출 ----

  function flash(c: CardEntry, key: string, apply: (on: boolean) => void, ms: number): void {
    clearTimeout(c.timers.get(key));
    apply(true);
    c.timers.set(
      key,
      setTimeout(() => {
        c.timers.delete(key);
        apply(false);
      }, ms),
    );
  }

  function toast(text: string): void {
    const t = h(doc, "div", "ow-panel__toast", text);
    toasts.append(t);
    const id = setTimeout(() => {
      toastTimers.delete(id);
      t.remove();
    }, TOAST_MS);
    toastTimers.add(id);
  }

  function applyCue(cue: CueDto): void {
    if (cue.kind !== "Shake" && cue.kind !== "Reject") return;
    const c = cards.get(cue.agent);
    if (!c) return;
    if (cue.kind === "Shake")
      flash(c, "hit", (on) => c.el.classList.toggle("is-hit", on), FLASH_MS);
    else flash(c, "reject", (on) => (c.ref.reject.hidden = !on), FLASH_MS);
  }

  function pick(agentId: string): void {
    const c = cards.get(agentId);
    if (!c) return;
    c.el.scrollIntoView({ block: "nearest" });
    flash(c, "hl", (on) => c.el.classList.toggle("is-highlight", on), HIGHLIGHT_MS);
  }

  // ---- 그리기 ----

  function nextTeamColor(): number {
    const used = new Set(teamColor.values());
    for (let n = 1; n <= TEAM_COLORS; n++) if (!used.has(n)) return n;
    return (teamColor.size % TEAM_COLORS) + 1;
  }

  function clear(): void {
    for (const c of cards.values()) {
      c.timers.forEach(clearTimeout);
      c.el.remove();
    }
    for (const hd of heads.values()) hd.el.remove();
    cards.clear();
    heads.clear();
    teamColor.clear();
    events.reset();
    ticker.setEvents([]);
    lastOfficeId = null;
    lastCueKey = "";
    count.hidden = true;
    officeName.textContent = "";
    empty.hidden = true;
  }

  function render(): void {
    const { office, list: info, selectedId } = store.state;
    // store 가 버린(선택이 바뀐) 상태는 그리지 않는다
    if (!office || office.officeId !== selectedId) {
      clear();
      return;
    }

    const small = info?.scale === "Small";
    // 처음 그리기와 조용한 재구성(PLAN 4.5)에는 토스트·연출을 내지 않는다
    const quiet = office.officeId !== lastOfficeId || office.rebuilt;
    lastOfficeId = office.officeId;

    const summary = info?.offices.find((o) => o.officeId === office.officeId);
    officeName.textContent = summary
      ? summary.building && summary.room !== null
        ? `${summary.building}동 ${summary.room}호`
        : summary.label
      : "";

    const teams = [...office.teams].sort(
      (a, b) => (Date.parse(b.lastActivity) || 0) - (Date.parse(a.lastActivity) || 0),
    );
    const all = new Map<string, EmployeeDto>();
    for (const t of teams) for (const e of t.employees) all.set(e.agentId, e);

    const desired: HTMLElement[] = [];
    const liveCards = new Set<string>();
    const liveHeads = new Set<string>();
    const fresh: EmployeeDto[] = [];

    for (const t of teams) {
      if (small) {
        let hd = heads.get(t.teamId);
        if (!hd) heads.set(t.teamId, (hd = createHead(doc)));
        if (!teamColor.has(t.teamId)) teamColor.set(t.teamId, nextTeamColor());
        updateHead(hd, t, teamColor.get(t.teamId) ?? 1);
        tickHead(hd, Date.now());
        liveHeads.add(t.teamId);
        desired.push(hd.el);
      }
      orderEmployees(t).forEach((e, i) => {
        let c = cards.get(e.agentId);
        if (!c) {
          cards.set(e.agentId, (c = createCard(doc, e.agentId)));
          c.el.classList.toggle("is-compact", compact);
          if (!quiet) fresh.push(e);
        }
        updateCard(c, e, i === 0, t.phase === "Off", small, all);
        tickCard(c, Date.now());
        liveCards.add(e.agentId);
        desired.push(c.el);
      });
    }

    for (const [id, c] of cards) {
      if (liveCards.has(id)) continue;
      c.timers.forEach(clearTimeout);
      c.el.remove();
      cards.delete(id);
    }
    for (const [id, hd] of heads) {
      if (liveHeads.has(id)) continue;
      hd.el.remove();
      heads.delete(id);
    }
    // 퇴근한 팀의 색은 반납한다
    for (const id of teamColor.keys()) if (!liveHeads.has(id)) teamColor.delete(id);

    // 옮기면 초점이 풀리는 브라우저가 있어 옮긴 뒤 되돌린다
    const focused = doc.activeElement;
    desired.forEach((el, i) => {
      if (list.children[i] !== el) list.insertBefore(el, list.children[i] ?? null);
    });
    if (focused instanceof HTMLElement && list.contains(focused) && doc.activeElement !== focused)
      focused.focus({ preventScroll: true });

    for (const e of fresh) {
      const c = cards.get(e.agentId);
      if (!c) continue;
      flash(c, "new", (on) => c.el.classList.toggle("is-new", on), NEW_MS);
      toast(fill(phrases.toast.hired, { name: e.name, rank: rankLabel(e.rank) }));
    }

    // 같은 상태(seq)가 다시 와도 연출은 한 번만
    const cueKey = `${office.officeId}|${office.stateSeq}`;
    if (cueKey !== lastCueKey) {
      lastCueKey = cueKey;
      if (!quiet) office.cues.forEach(applyCue);
    }

    count.hidden = false;
    count.textContent = fill(phrases.card.count, { n: all.size });
    empty.hidden = all.size > 0;
    if (events.ingest(office, small)) ticker.setEvents(events.events());
  }

  // ---- 창 크기 ----

  const win = doc.defaultView;
  const viewport: Viewport =
    opts.viewport ??
    (win
      ? {
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
        }
      : { size: () => ({ w: 1280, h: 800 }), subscribe: () => () => {} });

  function setDrawer(open: boolean): void {
    const next = narrow && open;
    if (next === drawerOpen) return;
    drawerOpen = next;
    applyLayout();
    opts.onDrawerChange?.(drawerOpen);
  }

  function applyLayout(): void {
    aside.classList.toggle("is-drawer", narrow && drawerOpen);
    aside.hidden = narrow && !drawerOpen;
    closeBtn.hidden = !narrow;
    ticker.el.hidden = compact;
    ticker.remeasure();
  }

  function onResize(): void {
    const { w, h: height } = viewport.size();
    const wasOpen = drawerOpen;
    narrow = w < DRAWER_BELOW;
    if (!narrow) drawerOpen = false;
    const nextCompact = height < COMPACT_BELOW;
    if (nextCompact !== compact) {
      compact = nextCompact;
      for (const c of cards.values()) c.el.classList.toggle("is-compact", compact);
    }
    applyLayout();
    if (wasOpen !== drawerOpen) opts.onDrawerChange?.(drawerOpen);
  }

  closeBtn.addEventListener("click", () => setDrawer(false));
  list.addEventListener("keydown", (ev) => {
    if (ev.key !== "ArrowUp" && ev.key !== "ArrowDown") return;
    const all = [...list.querySelectorAll<HTMLElement>(".ow-card")];
    const at = all.findIndex((c) => c === doc.activeElement);
    const next = all[at + (ev.key === "ArrowDown" ? 1 : -1)];
    if (at < 0 || !next) return;
    ev.preventDefault();
    next.focus();
  });
  const unsubscribe = store.subscribe(render);
  const unsubscribeViewport = viewport.subscribe(onResize);
  // 경과 시간은 1초마다 그 칸만 (상태를 매초 다시 받지 않는다)
  const tick = setInterval(() => {
    const now = Date.now();
    for (const c of cards.values()) tickCard(c, now);
    for (const hd of heads.values()) tickHead(hd, now);
  }, 1000);

  onResize();
  render();

  return {
    openDrawer: () => setDrawer(true),
    toggleDrawer: () => setDrawer(!drawerOpen),
    destroy() {
      unsubscribe();
      unsubscribeViewport();
      clearInterval(tick);
      toastTimers.forEach(clearTimeout);
      for (const c of cards.values()) c.timers.forEach(clearTimeout);
      aside.remove();
    },
  };
}
