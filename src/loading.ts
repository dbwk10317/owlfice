// 로딩 화면 (PLAN 3.8, HANDOFF 4.7). 백엔드의 startup-progress 를 받아 건물 창문을 켜고 단계 목록을 그린다.
// 문구 틀은 phrases.ko.json 의 loading 이 갖고, 여기서는 백엔드가 준 재료로 채우기만 한다.
import "./styles/loading.css";
import phrases from "../design/handoff/phrases.ko.json";
import type { Invoke, Sheet } from "./scale.ts";
import type { StartupDetail, StartupProgressDto, StartupSummary } from "./types/ipc.ts";

/** ui.json 의 owlfice.building 에서 쓰는 것만 */
export type BuildingMeta = {
  windows: [number, number][];
  lightOrder: number[];
  signOffset: [number, number];
};
export type LoadingSheet = Sheet & { building?: BuildingMeta };

type Handler = (e: { payload: StartupProgressDto }) => void;
export type LoadingIo = {
  invoke: Invoke;
  listen: (event: string, handler: Handler) => Promise<() => void>;
  /** 문구 풀에서 고르는 난수. 테스트가 바꾼다 */
  random?: () => number;
};

const L = phrases.loading;
const STEP_COUNT = L.steps.length;
/** 건물은 2배 (HANDOFF 4.7) */
const ZOOM = 2;
/** 전체가 아무리 빨리 끝나도 이만큼은 보여 준다 (3.8) */
const MIN_MS = 1200;
const CAUSE_AFTER_MS = 2000;
const SLOW_AFTER_MS = 5000;
/** 말줄임표 점 하나가 늘어나는 간격 */
const DOT_MS = 600;
/** 간판 네온이 켜진 뒤 메인으로 넘어가기까지 (HANDOFF 4.7) */
const SIGN_MS = 300;
const FADE_MS = 300;
/** 아직 안 켜진 창 중 올빼미 눈이 뜨이는 수 */
const EYES = 3;
const TICK_MS = 100;
/** 건너뛰기에서 빼는 보조 키 (HANDOFF 10) */
const MODIFIER_KEYS = new Set([
  "Tab",
  "Shift",
  "Control",
  "Alt",
  "AltGraph",
  "Meta",
  "OS",
  "CapsLock",
  "NumLock",
  "ScrollLock",
  "Fn",
  "FnLock",
]);

const CHECK = '<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M5 12l5 5 9-10"/></svg>';
const WARN = '<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M12 7v6M12 17h.01"/></svg>';

type RowState = "todo" | "running" | "done" | "error";
type Row = {
  state: RowState;
  summary: StartupSummary | null;
  detail: StartupDetail | null;
  /** 이 줄이 진행 중이 된 때 */
  since: number;
  phrase: string;
};
type Note = { step: number; el: HTMLElement };
type Building = { box: HTMLElement; update(litSteps: number, signOn: boolean): void };

function fill(tpl: string, vars: Record<string, string | number>): string {
  return tpl.replace(/\{(\w+)\}/g, (m, k: string) => (k in vars ? String(vars[k]) : m));
}

function el(tag: string, cls: string, text?: string): HTMLElement {
  const e = document.createElement(tag);
  e.className = cls;
  if (text !== undefined) e.textContent = text;
  return e;
}

function isNote(s: StartupSummary): boolean {
  return s.kind === "WslOff" || s.kind === "WslFail" || s.kind === "SettingsBroken";
}

function doneText(step: number, s: StartupSummary, company: string): string {
  const tpl = L.steps[step - 1].done;
  switch (s.kind) {
    case "Candidates":
      return fill(tpl, { n: s.n, hosts: s.hosts.join(", ") });
    case "Rented":
      return fill(tpl, { buildings: s.buildings.join(" · ") });
    case "Teams":
      return fill(tpl, { n: s.n });
    case "Hired":
      return fill(tpl, { n: s.n, b: s.bujang, g: s.gwajang, d: s.daeri, s: s.sawon });
    default:
      // 1단계 "사명: {company}". 다른 단계 틀에는 이 칸이 없다
      return fill(tpl, { company });
  }
}

function noteText(s: StartupSummary): string {
  switch (s.kind) {
    case "WslOff":
      return fill(L.wslOff, { floor: s.floor, distro: s.distro });
    case "WslFail":
      return fill(L.wslFail, { floor: s.floor, distro: s.distro });
    case "SettingsBroken":
      return phrases.empty.settingsBroken;
    default:
      return "";
  }
}

function causeText(d: StartupDetail | null): string | null {
  if (!d) return null;
  return d.kind === "Files"
    ? fill(L.cause.files, { total: d.total, done: d.done })
    : fill(L.cause.wsl, { distro: d.distro });
}

/** 줄 모양이 같으면 다시 그리지 않는다. 읽어 주는 화면(aria-live)이 같은 줄을 되풀이하지 않게 */
function paint(row: HTMLElement, sig: string, build: () => HTMLElement[], cls: string): void {
  if (row.dataset.sig === sig) return;
  row.dataset.sig = sig;
  row.className = `ld__step ${cls}`;
  row.replaceChildren(...build());
}

function mark(kind: "done" | "off" | "warn" | "todo"): HTMLElement {
  const m = el("span", `ld__mk ld__mk--${kind}`);
  m.setAttribute("aria-hidden", "true");
  if (kind === "done" || kind === "off") m.innerHTML = CHECK;
  if (kind === "warn") m.innerHTML = WARN;
  return m;
}

/** 끝난 줄: 본문 뒤 괄호는 흐린 글자 (시안) */
function lineText(text: string): HTMLElement {
  const t = el("span", "ld__text");
  const m = /^(.*?)( \(.*\))$/.exec(text);
  if (!m) {
    t.textContent = text;
    return t;
  }
  t.append(m[1], el("span", "ld__paren", m[2]));
  return t;
}

export function reducedMotion(): boolean {
  return typeof matchMedia === "function" && matchMedia("(prefers-reduced-motion: reduce)").matches;
}

/** 건물 위에 얹는 그림 한 조각 (원본 좌표 x, y, 2배). 장식이라 읽어 주지 않는다 */
function piece(sheet: LoadingSheet, name: string, x: number, y: number): HTMLElement | null {
  const f = sheet.frames[name]?.frame;
  if (!f) return null;
  const e = el("div", "ld__px");
  e.setAttribute("aria-hidden", "true");
  e.style.cssText =
    `left:${x * ZOOM}px;top:${y * ZOOM}px;width:${f.w * ZOOM}px;height:${f.h * ZOOM}px;` +
    `background:url(/ui.png) -${f.x * ZOOM}px -${f.y * ZOOM}px/${sheet.size.w * ZOOM}px ${sheet.size.h * ZOOM}px no-repeat`;
  return e;
}

/** 건물: 단계가 끝날 때마다 lightOrder 순서로 창에 불이 켜지고, 7단계가 끝나면 간판 네온이 들어온다 */
function buildBuilding(sheet: LoadingSheet | undefined): Building {
  const box = el("div", "ld__building");
  const meta = sheet?.building;
  const base = sheet && piece(sheet, "building/base", 0, 0);
  if (!sheet || !meta || !base) return { box, update() {} };
  box.append(base);
  // 창마다 불과 눈 조각을 하나씩 만들어 두고 켜짐만 바꾼다
  const lit = new Map<number, HTMLElement>();
  const eyes = new Map<number, HTMLElement>();
  meta.windows.forEach(([x, y], i) => {
    const l = piece(sheet, "building/window_lit", x, y);
    const e = piece(sheet, "building/window_eyes", x, y);
    for (const [map, p] of [
      [lit, l],
      [eyes, e],
    ] as const) {
      if (!p) continue;
      p.classList.add("ld__win");
      box.append(p);
      map.set(i, p);
    }
  });
  const sign = piece(sheet, "building/sign_on", meta.signOffset[0], meta.signOffset[1]);
  if (sign) {
    sign.classList.add("ld__win");
    box.append(sign);
  }
  return {
    box,
    update(litSteps: number, signOn: boolean) {
      const on = Math.round((litSteps / STEP_COUNT) * meta.lightOrder.length);
      meta.lightOrder.forEach((w, rank) => {
        lit.get(w)?.classList.toggle("is-on", rank < on);
        eyes.get(w)?.classList.toggle("is-on", rank >= on && rank < on + EYES && !signOn);
      });
      sign?.classList.toggle("is-on", signOn);
    },
  };
}

/**
 * 화면을 그리고 7단계가 끝나(또는 건너뛰어) 메인으로 넘어갈 때 풀린다. sheet 가 없으면 건물 없이 글만.
 * companyName 은 1단계 완료 줄의 사명. 없으면 기본 이름
 */
export function showLoading(
  root: HTMLElement,
  io: LoadingIo,
  sheet?: LoadingSheet,
  companyName?: string,
): Promise<void> {
  const company = companyName ?? phrases.app.brand;
  const random = io.random ?? Math.random;
  const start = Date.now();
  const rows: Row[] = L.steps.map((s) => ({
    state: "todo",
    summary: null,
    detail: null,
    since: start,
    phrase: s.pool[Math.floor(random() * s.pool.length)],
  }));
  const notes = new Map<string, Note>();

  const page = el("main", "ld");
  const building = buildBuilding(sheet);
  const list = el("section", "ld__steps");
  list.setAttribute("aria-live", "polite");
  const stepEls = rows.map(() => el("div", "ld__step"));
  list.append(...stepEls);
  const foot = el("p", "ld__foot", `${L.skip} `);
  foot.append(el("span", "ow-kbd", L.skipKey));
  page.append(building.box, list, foot);
  root.replaceChildren(page);

  // 콜백에서도 바뀌는 값이라 타입 검사기가 "run" 으로 굳히지 않게 넓혀 둔다
  type Phase = "run" | "sign" | "fade" | "done";
  let phase = "run" as Phase;
  let finish!: () => void;
  const done = new Promise<void>((resolve) => (finish = resolve));
  let signTimer: ReturnType<typeof setTimeout> | undefined;
  let fadeTimer: ReturnType<typeof setTimeout> | undefined;
  let unlisten: (() => void) | undefined;

  function paintRow(i: number): void {
    const r = rows[i];
    const step = i + 1;
    const dim = r.state === "todo" ? "ld__step--todo" : "";
    switch (r.state) {
      case "todo":
        paint(
          stepEls[i],
          `todo|${r.phrase}`,
          () => [mark("todo"), el("span", "ld__text", r.phrase)],
          dim,
        );
        break;
      case "running": {
        const elapsed = Date.now() - r.since;
        const dots = ".".repeat(Math.min(3, 1 + Math.floor(elapsed / DOT_MS)));
        const cause = elapsed >= CAUSE_AFTER_MS ? causeText(r.detail) : null;
        const slow = elapsed >= SLOW_AFTER_MS;
        paint(
          stepEls[i],
          `run|${r.phrase}|${dots}|${cause}|${slow}`,
          () => {
            const spin = el("span", "ld__spin");
            spin.setAttribute("aria-hidden", "true");
            const body = el("span", "ld__body");
            body.append(el("b", "ld__now", r.phrase + dots));
            if (cause) body.append(el("span", "ld__cause", cause));
            if (slow) body.append(el("span", "ld__slow", L.slow));
            return [spin, body];
          },
          "ld__step--running",
        );
        break;
      }
      case "done":
      case "error": {
        const s = r.summary ?? { kind: "None" as const };
        const failed = r.state === "error";
        // 세션 폴더를 하나도 못 찾은 3단계는 임대 문구 대신 3.9 의 말을 쓴다
        const text =
          s.kind === "NoFolders" ? phrases.empty.noFoldersTitle : doneText(step, s, company);
        paint(
          stepEls[i],
          `${r.state}|${text}`,
          () => [mark(failed ? "warn" : "done"), lineText(text)],
          failed ? "ld__step--warn" : "ld__step--done",
        );
      }
    }
  }

  function paintAll(): void {
    rows.forEach((_, i) => paintRow(i));
    // 낮은 창에서는 처음과 끝에서도 3줄을 유지한다 (HANDOFF 반응형 "3줄만")
    const cur = Math.min(
      rows.findIndex((r) => r.state !== "done" && r.state !== "error") + 1 || STEP_COUNT,
      STEP_COUNT,
    );
    const start = Math.max(1, Math.min(cur - 1, STEP_COUNT - 2));
    const near = (step: number) => step >= start && step < start + 3;
    stepEls.forEach((e, i) => e.toggleAttribute("data-near", near(i + 1)));
    for (const n of notes.values()) n.el.toggleAttribute("data-near", near(n.step));
    const ended = rows.filter((r) => r.state === "done" || r.state === "error").length;
    building.update(ended, phase !== "run");
  }

  function upsertNote(p: StartupProgressDto): void {
    const key = JSON.stringify([p.step, p.summary]);
    let n = notes.get(key);
    if (!n) {
      const e = el("div", "ld__step");
      // 같은 단계의 마지막 줄 뒤에 붙인다
      const mine = [...notes.values()].filter((x) => x.step === p.step);
      const after = mine[mine.length - 1]?.el;
      (after ?? stepEls[p.step - 1]).after(e);
      n = { step: p.step, el: e };
      notes.set(key, n);
    }
    const text = noteText(p.summary);
    // 꺼진 배포판은 경고가 아니라 소등이다. 접근 실패와 손상 설정만 노란 경고
    const warn = p.state === "Warn" || p.state === "Error";
    paint(
      n.el,
      `${warn}|${text}`,
      () => [mark(warn ? "warn" : "off"), lineText(text)],
      warn ? "ld__step--warn ld__step--note" : "ld__step--off ld__step--note",
    );
  }

  /** 이벤트로 이미 받은 단계. 다시 불러온 기록은 이 줄을 덮지 않는다 (기록이 이벤트보다 오래됐을 수 있다) */
  const live = new Set<number>();

  function apply(p: StartupProgressDto, replay = false): void {
    if (p.step < 1 || p.step > STEP_COUNT) return;
    if (isNote(p.summary)) {
      upsertNote(p);
    } else {
      if (replay && live.has(p.step)) return;
      if (!replay) live.add(p.step);
      const r = rows[p.step - 1];
      // 끝난 줄은 늦게 도착한 옛 이벤트로 되돌리지 않는다 (구독과 다시 불러오기가 겹칠 수 있다)
      if (r.state === "done" || r.state === "error") return;
      if (p.state === "Running") {
        if (r.state !== "running") {
          r.state = "running";
          r.since = Date.now();
        }
        r.detail = p.detail;
      } else {
        r.state = p.state === "Done" ? "done" : "error";
        r.summary = p.summary;
        r.detail = null;
      }
    }
    paintAll();
    check();
  }

  function cleanup(): void {
    clearInterval(ticker);
    clearTimeout(signTimer);
    clearTimeout(fadeTimer);
    document.removeEventListener("keydown", onKey);
    document.removeEventListener("click", skip);
    unlisten?.();
  }

  function fade(): void {
    if (phase === "fade" || phase === "done") return;
    phase = "fade";
    clearTimeout(signTimer);
    paintAll();
    if (reducedMotion()) {
      phase = "done";
      cleanup();
      finish();
      return;
    }
    page.classList.add("ld--fade");
    fadeTimer = setTimeout(() => {
      phase = "done";
      cleanup();
      finish();
    }, FADE_MS);
  }

  function check(): void {
    if (phase !== "run") return;
    const ended = rows.every((r) => r.state === "done" || r.state === "error");
    if (!ended || Date.now() - start < MIN_MS) return;
    phase = "sign";
    paintAll();
    signTimer = setTimeout(fade, SIGN_MS);
  }

  function skip(): void {
    fade();
  }

  function onKey(e: KeyboardEvent): void {
    // 규모 선택 카드를 Enter 로 누른 채면 자동 반복 keydown 이 이 화면으로 넘어와 곧바로 건너뛴다
    if (!e.repeat && !MODIFIER_KEYS.has(e.key)) skip();
  }

  const ticker = setInterval(() => {
    paintAll();
    check();
  }, TICK_MS);
  document.addEventListener("keydown", onKey);
  document.addEventListener("click", skip);
  paintAll();

  // 구독을 먼저 걸고 쌓인 기록을 다시 받는다: 앞 단계는 화면이 뜨기 전에 이미 끝났을 수 있다 (PLAN 4.3 시작 순서)
  void (async () => {
    try {
      unlisten = await io.listen("startup-progress", (e) => apply(e.payload));
      if (phase === "done") unlisten();
    } catch (e) {
      console.error("startup-progress 구독", e);
    }
    try {
      (await io.invoke<StartupProgressDto[]>("get_startup_progress")).forEach((p) =>
        apply(p, true),
      );
    } catch (e) {
      console.error("get_startup_progress", e);
    }
  })();

  return done;
}
