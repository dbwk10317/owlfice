// @vitest-environment jsdom
import { afterEach, expect, test } from "vite-plus/test";
import phrases from "../design/handoff/phrases.ko.json";
import { detailErrorText, openModal } from "./popup-modal.ts";

afterEach(() => {
  document.body.innerHTML = "";
});

const press = (key: string) =>
  document.dispatchEvent(new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true }));
const opened = () => [...document.querySelectorAll(".ow-backdrop")];

function opener(): HTMLButtonElement {
  const b = document.createElement("button");
  document.body.append(b);
  b.focus();
  return b;
}

test("Esc 로 닫고 초점은 연 요소로 돌아간다", () => {
  const b = opener();
  const m = openModal(document, { width: 780, titleId: "t" });
  expect(opened()).toHaveLength(1);
  expect(document.activeElement).toBe(m.modal);
  press("Escape");
  expect(opened()).toHaveLength(0);
  expect(m.closed).toBe(true);
  expect(document.activeElement).toBe(b);
});

test("닫기 버튼과 바깥 클릭으로 닫고, 안쪽 클릭은 닫지 않는다", () => {
  opener();
  const m = openModal(document, { width: 780, titleId: "t" });
  m.modal.append(m.closeBtn);
  m.modal.dispatchEvent(new MouseEvent("mousedown", { bubbles: true }));
  expect(opened()).toHaveLength(1);
  document.querySelector(".ow-backdrop")!.dispatchEvent(new MouseEvent("mousedown"));
  expect(opened()).toHaveLength(0);

  const again = openModal(document, { width: 780, titleId: "t" });
  again.modal.append(again.closeBtn);
  again.closeBtn.click();
  expect(opened()).toHaveLength(0);
});

test("팝업은 한 번에 하나, 두 번째가 닫힐 때 처음 연 요소로 돌아간다", () => {
  const b = opener();
  const first = openModal(document, { width: 780, titleId: "a" });
  const second = openModal(document, { width: 940, titleId: "b" });
  expect(first.closed).toBe(true);
  expect(opened()).toHaveLength(1);
  second.close();
  expect(document.activeElement).toBe(b);
});

test("오류 코드마다 문구가 있고 모르는 값은 그대로 보인다", () => {
  for (const code of [
    "not_found",
    "cursor_invalid",
    "host_offline",
    "timeout",
    "open_failed",
  ] as const)
    expect(detailErrorText(code)).toBe(phrases.detailError[code]);
  expect(detailErrorText("뭔가 이상함")).toBe("뭔가 이상함");
});

test("Tab 가두기는 조상이 hidden 인 버튼을 마지막으로 잡지 않는다", () => {
  const b = opener();
  const m = openModal(document, { width: 780, titleId: "t" });
  const visible = document.createElement("button");
  const folded = document.createElement("div");
  folded.hidden = true;
  const inner = document.createElement("button");
  folded.append(inner);
  m.modal.append(m.closeBtn, visible, folded);
  visible.focus();
  const ev = new KeyboardEvent("keydown", { key: "Tab", bubbles: true, cancelable: true });
  document.dispatchEvent(ev);
  expect(ev.defaultPrevented).toBe(true); // 마지막(visible)에서 첫 칸으로 돌린다
  expect(document.activeElement).toBe(m.closeBtn);
  b.remove();
});

test("입력·선택·여러 줄 입력도 초점을 가두고 disabled fieldset은 건너뛴다", () => {
  const m = openModal(document, { width: 580, titleId: "t" });
  const input = document.createElement("input");
  const select = document.createElement("select");
  const textarea = document.createElement("textarea");
  const disabled = document.createElement("fieldset");
  disabled.disabled = true;
  disabled.append(document.createElement("input"));
  m.modal.append(input, select, textarea, disabled);
  textarea.focus();
  press("Tab");
  expect(document.activeElement).toBe(input);
  document.dispatchEvent(
    new KeyboardEvent("keydown", { key: "Tab", shiftKey: true, cancelable: true }),
  );
  expect(document.activeElement).toBe(textarea);
  m.close();
});
