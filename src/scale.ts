// 첫 실행의 규모 선택 화면 (HANDOFF 4.7). 고르기 전에는 끝나지 않는다.
import "./scale.css";
import phrases from "../design/handoff/phrases.ko.json";
import type { Scale, SettingsDto } from "./types/ipc.ts";

export type Invoke = <T>(cmd: string, args?: Record<string, unknown>) => Promise<T>;

/** ui.json 의 frames·meta.size 에서 쓰는 것만 */
export type Frames = Record<string, { frame: { x: number; y: number; w: number; h: number } }>;
export type Sheet = { frames: Frames; size: { w: number; h: number } };

const ORDER: Scale[] = ["Large", "Small"];

function sprite(sheet: Sheet | undefined, name: string, zoom: number): HTMLElement | null {
  const f = sheet?.frames[name]?.frame;
  if (!f) return null;
  const el = document.createElement("div");
  el.className = "sc__px";
  el.setAttribute("aria-hidden", "true");
  el.style.cssText =
    `width:${f.w * zoom}px;height:${f.h * zoom}px;background:url(/ui.png) ` +
    `-${f.x * zoom}px -${f.y * zoom}px/${sheet!.size.w * zoom}px ${sheet!.size.h * zoom}px no-repeat`;
  return el;
}

function el(tag: string, cls: string, text?: string): HTMLElement {
  const e = document.createElement(tag);
  e.className = cls;
  if (text) e.textContent = text;
  return e;
}

export type ScaleChoice = { scale: Scale; companyName: string | null };

/** 화면을 그리고 고른 규모와 입력한 회사 이름(비면 null)으로 풀린다. sheet 가 없으면 그림 없이 글만 */
export function showScale(root: HTMLElement, invoke: Invoke, sheet?: Sheet): Promise<ScaleChoice> {
  const t = phrases.onboarding;
  const { app } = phrases;
  let finish!: (choice: ScaleChoice) => void;
  const done = new Promise<ScaleChoice>((resolve) => (finish = resolve));
  // 저장이 실패해도 백엔드는 이번 실행에 반영하므로 그대로 진행한다
  async function choose(scale: Scale): Promise<void> {
    const dto: SettingsDto = { scale, companyName: input.value.trim() || null };
    try {
      await invoke("set_settings", { dto });
    } catch (e) {
      console.error("set_settings", e);
    }
    finish({ scale, companyName: dto.companyName });
  }
  const page = el("main", "sc");

  const brand = el("div", "sc__brand");
  const logo = sprite(sheet, "logo/default", 2);
  if (logo) brand.append(logo);
  brand.append(el("b", "", app.brand), el("span", "", app.brandSub));

  const head = el("div", "sc__head");
  const title = el("h1", "", t.title);
  title.id = "sc-title";
  head.append(title, el("p", "", t.body));

  // 폼이 아니라 Enter 는 아무것도 하지 않는다. 규모를 골라야 끝난다
  const company = el("label", "sc__company", t.companyLabel);
  const input = document.createElement("input");
  input.type = "text";
  input.maxLength = 20;
  input.placeholder = app.brand;
  input.autocomplete = "organization";
  company.append(input);

  const group = el("div", "sc__opts");
  group.setAttribute("role", "group");
  group.setAttribute("aria-labelledby", "sc-title");
  const buttons = ORDER.map((scale) => {
    const text = scale === "Large" ? t.large : t.small;
    const b = el("button", "sc__opt") as HTMLButtonElement;
    b.type = "button";
    const pv = el("span", "sc__pv");
    const img = sprite(sheet, scale === "Large" ? "preview/large" : "preview/small", 2);
    if (img) pv.append(img);
    const txt = el("span", "sc__txt");
    txt.append(el("span", "sc__name", text.title), el("span", "sc__line", text.line));
    txt.append(el("span", "sc__fit", text.fit));
    b.append(pv, txt);
    b.addEventListener("click", () => {
      buttons.forEach((x) => (x.disabled = true));
      void choose(scale);
    });
    return b;
  });
  group.append(...buttons);

  const building = sprite(sheet, "building/base", 2);
  building?.classList.add("sc__building");
  page.append(
    brand,
    ...(building ? [building] : []),
    head,
    company,
    group,
    el("p", "sc__foot", t.footnote),
  );
  root.replaceChildren(page);
  input.focus();
  return done;
}
