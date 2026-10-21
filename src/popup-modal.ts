// 팝업 두 종류가 같이 쓰는 모달 껍데기 (HANDOFF 4.6): 바탕, Esc·바깥 클릭으로 닫기, 초점 가두기와 되돌리기.
// 팝업은 한 번에 하나다.
import "../design/handoff/tokens.css";
import "./styles/popup.css";
import phrases from "../design/handoff/phrases.ko.json";
import closeSvg from "../design/handoff/icons/close.svg?raw";
import { h, icon } from "./sidebar-right.ts";
import type { EmployeeDto } from "./types/ipc.ts";

/** 지금 화면에 있는 직원. 팝업이 이름·방·컨텍스트를 찾는 길 (없으면 undefined) */
export type LiveEmployee = (teamId: string, agentId: string) => EmployeeDto | undefined;

/** 작은 아이콘 (14) */
export function smallIcon(doc: Document, svg: string): HTMLElement {
  const i = icon(doc, svg);
  i.classList.add("ow-icon--sm");
  return i;
}

/** HH:MM:SS (지역 시간) */
export function hms(iso: string): string {
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return "";
  return [d.getHours(), d.getMinutes(), d.getSeconds()]
    .map((n) => String(n).padStart(2, "0"))
    .join(":");
}

const ERRORS = phrases.detailError as Record<string, string>;
/** 상세 커맨드가 거절한 코드(DetailError)의 문구. 모르는 값은 그대로 보인다 */
export const detailErrorText = (e: unknown): string => ERRORS[String(e)] ?? String(e);

export interface Modal {
  modal: HTMLElement;
  closeBtn: HTMLElement;
  /** 연 요소. 닫으면 초점이 돌아간다 */
  opener: HTMLElement | null;
  closed: boolean;
  close(restoreFocus?: boolean): void;
}

let current: Modal | null = null;

const FOCUSABLE = 'button:not([disabled]), a[href], [tabindex]:not([tabindex="-1"])';

/** `width` 는 720 이상에서만 쓴다. 그 밑은 tokens.css 의 전체 화면 규칙이 이긴다 */
export function openModal(
  doc: Document,
  o: { width: number; titleId: string; opener?: HTMLElement | null },
): Modal {
  let opener = o.opener ?? (doc.activeElement as HTMLElement | null);
  // 열린 팝업 안에서 다시 열면 처음 연 요소로 돌아가야 한다
  if (current && opener && current.modal.contains(opener)) opener = current.opener;
  current?.close(false);

  const backdrop = h(doc, "div", "ow-backdrop");
  const modal = h(doc, "section", "ow-modal");
  modal.setAttribute("role", "dialog");
  modal.setAttribute("aria-modal", "true");
  modal.setAttribute("aria-labelledby", o.titleId);
  modal.tabIndex = -1;
  modal.style.setProperty("--ow-modal-w", `${o.width}px`);
  backdrop.append(modal);

  const closeBtn = h(doc, "button", "ow-iconbtn ow-modal__close");
  closeBtn.setAttribute("type", "button");
  closeBtn.setAttribute("aria-label", phrases.sessionDetail.close);
  closeBtn.append(icon(doc, closeSvg));

  const self: Modal = {
    modal,
    closeBtn,
    opener,
    closed: false,
    close(restoreFocus = true) {
      if (self.closed) return;
      self.closed = true;
      if (current === self) current = null;
      doc.removeEventListener("keydown", onKey, true);
      backdrop.remove();
      if (restoreFocus && opener?.isConnected) opener.focus();
    },
  };

  function onKey(e: KeyboardEvent): void {
    if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      self.close();
      return;
    }
    if (e.key !== "Tab") return;
    // 접힌 행처럼 조상이 hidden 인 것도 뺀다
    const items = [...modal.querySelectorAll<HTMLElement>(FOCUSABLE)].filter(
      (el) => !el.closest("[hidden]"),
    );
    const first = items[0];
    const last = items[items.length - 1];
    if (!first || !last) return;
    const at = doc.activeElement;
    if (!modal.contains(at)) {
      e.preventDefault();
      first.focus();
    } else if (e.shiftKey && (at === first || at === modal)) {
      e.preventDefault();
      last.focus();
    } else if (!e.shiftKey && at === last) {
      e.preventDefault();
      first.focus();
    }
  }
  doc.addEventListener("keydown", onKey, true);
  // mousedown 으로 본다: 글자를 끌어 고르다 바깥에서 놓아도 click 은 바탕에 서기 때문
  backdrop.addEventListener("mousedown", (e) => {
    if (e.target === backdrop) self.close();
  });
  closeBtn.addEventListener("click", () => self.close());

  // 버튼 초기화(.ow-app button)와 초점 고리가 .ow-app 안에서만 걸린다
  (doc.querySelector(".ow-app") ?? doc.body).append(backdrop);
  modal.focus();
  current = self;
  return self;
}
