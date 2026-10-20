// 첫 실행의 규모 선택 화면 (HANDOFF 4.7). 고르기 전에는 끝나지 않는다.
import "./scale.css";
import phrases from "../design/handoff/phrases.ko.json";
import type { Scale, SettingsDto } from "./types/ipc.ts";

type Invoke = <T>(cmd: string, args?: Record<string, unknown>) => Promise<T>;

/** ui.json 의 frames 에서 쓰는 칸만 */
export type Frames = Record<string, { frame: { x: number; y: number; w: number; h: number } }>;

const ORDER: Scale[] = ["Large", "Small"];

/** 카드 사이 방향키 이동. 끝에서는 멈춘다 */
export function moveFocus(index: number, key: string): number {
  const next = key === "ArrowRight" ? index + 1 : key === "ArrowLeft" ? index - 1 : index;
  return Math.min(ORDER.length - 1, Math.max(0, next));
}

/** 숫자 키로 바로 고르기 (1 대기업, 2 중소기업). Enter·Space 는 카드(button)가 한다 */
export function keyChoice(key: string): Scale | null {
  return ORDER[Number(key) - 1] ?? null;
}

/** 규모를 한 번만 받아 저장한다. 저장이 실패해도 백엔드는 이번 실행에 반영하므로 그대로 진행한다 */
export function createPicker(invoke: Invoke) {
  let chosen = false;
  let finish!: (scale: Scale) => void;
  const done = new Promise<Scale>((resolve) => (finish = resolve));
  async function choose(scale: Scale): Promise<void> {
    if (chosen) return;
    chosen = true;
    const dto: SettingsDto = { scale };
    try {
      await invoke("set_settings", { dto });
    } catch (e) {
      console.error("set_settings", e);
    }
    finish(scale);
  }
  return { choose, done };
}

function sprite(frames: Frames, name: string, zoom: number, label: string): HTMLElement | null {
  const f = frames[name]?.frame;
  if (!f) return null;
  const el = document.createElement("div");
  el.className = "sc__px";
  el.setAttribute("role", "img");
  el.setAttribute("aria-label", label);
  el.style.cssText =
    `width:${f.w * zoom}px;height:${f.h * zoom}px;background:url(/ui.png) ` +
    `-${f.x * zoom}px -${f.y * zoom}px/${512 * zoom}px ${256 * zoom}px no-repeat`;
  return el;
}

function el(tag: string, cls: string, text?: string): HTMLElement {
  const e = document.createElement(tag);
  e.className = cls;
  if (text) e.textContent = text;
  return e;
}

/** 화면을 그리고 고른 규모로 풀린다. frames 가 없으면 그림 없이 글만 */
export function showScale(root: HTMLElement, invoke: Invoke, frames: Frames = {}): Promise<Scale> {
  const t = phrases.onboarding;
  const picker = createPicker(invoke);
  const page = el("main", "sc");

  const brand = el("div", "sc__brand");
  const logo = sprite(frames, "logo/default", 2, "");
  if (logo) brand.append(logo);
  brand.append(el("b", "", "Owlfice"), el("span", "", "야근불"));

  const head = el("div", "sc__head");
  const title = el("h1", "", t.title);
  title.id = "sc-title";
  head.append(title, el("p", "", t.body));

  const group = el("div", "sc__opts");
  group.setAttribute("role", "group");
  group.setAttribute("aria-labelledby", "sc-title");
  const buttons = ORDER.map((scale) => {
    const text = scale === "Large" ? t.large : t.small;
    const b = el("button", "sc__opt") as HTMLButtonElement;
    b.type = "button";
    const pv = el("span", "sc__pv");
    const img = sprite(frames, scale === "Large" ? "preview/large" : "preview/small", 2, "");
    if (img) pv.append(img);
    const txt = el("span", "sc__txt");
    txt.append(el("span", "sc__name", text.title), el("span", "sc__line", text.line));
    txt.append(el("span", "sc__fit", text.fit));
    b.append(pv, txt);
    b.addEventListener("click", () => {
      buttons.forEach((x) => (x.disabled = true));
      void picker.choose(scale);
    });
    return b;
  });
  group.append(...buttons);
  group.addEventListener("keydown", (e) => {
    const pick = keyChoice(e.key);
    if (pick) {
      buttons[ORDER.indexOf(pick)].click();
      return;
    }
    const at = buttons.indexOf(document.activeElement as HTMLButtonElement);
    if (at >= 0) buttons[moveFocus(at, e.key)].focus();
  });

  const building = sprite(frames, "building/base", 2, "불 꺼진 Owlfice 건물");
  building?.classList.add("sc__building");
  page.append(brand, ...(building ? [building] : []), head, group, el("p", "sc__foot", t.footnote));
  root.replaceChildren(page);
  buttons[0].focus();
  return picker.done;
}
