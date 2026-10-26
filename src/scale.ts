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
  let confirmed: ScaleChoice | null = null;
  async function choose(scale: Scale): Promise<void> {
    error.hidden = true;
    continueBtn.hidden = true;
    confirmed = null;
    const name = input.value.trim();
    if (Array.from(name).length > 20) {
      error.textContent = "회사 이름은 20자까지 입력할 수 있습니다.";
      error.hidden = false;
      buttons.forEach((b) => (b.disabled = false));
      input.focus();
      return;
    }
    const dto: Pick<SettingsDto, "scale" | "companyName"> = {
      scale,
      companyName: name || null,
    };
    try {
      await invoke("set_settings", { dto });
      finish({ scale, companyName: dto.companyName });
    } catch {
      try {
        const saved = await invoke<SettingsDto>("get_settings");
        if (saved.scale === scale) {
          confirmed = { scale: saved.scale, companyName: saved.companyName };
          error.textContent = "저장하지 못했습니다. 이번 실행에는 선택한 규모가 반영됐습니다.";
          continueBtn.hidden = false;
        } else {
          error.textContent = "저장하지 못했습니다. 규모를 다시 선택해 주세요.";
          buttons.forEach((b) => (b.disabled = false));
        }
      } catch {
        error.textContent = "설정을 확인하지 못했습니다. 규모를 다시 선택해 주세요.";
        buttons.forEach((b) => (b.disabled = false));
      }
      error.hidden = false;
    }
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
  const error = el("p", "sc__error");
  error.setAttribute("role", "alert");
  error.hidden = true;
  const continueBtn = el("button", "sc__continue", "이번 실행에서 계속") as HTMLButtonElement;
  continueBtn.type = "button";
  continueBtn.hidden = true;
  continueBtn.addEventListener("click", () => {
    if (confirmed) finish(confirmed);
  });

  const building = sprite(sheet, "building/base", 2);
  building?.classList.add("sc__building");
  page.append(
    brand,
    ...(building ? [building] : []),
    head,
    company,
    group,
    error,
    continueBtn,
    el("p", "sc__foot", t.footnote),
  );
  root.replaceChildren(page);
  input.focus();
  return done;
}
