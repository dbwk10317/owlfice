// @vitest-environment jsdom
import { afterEach, expect, test, vi } from "vite-plus/test";
import { openSettings, saveSettings } from "./popup-settings.ts";
import type { Invoke } from "./scale.ts";
import type { SettingsDto } from "./types/ipc.ts";

const settle = () => new Promise((resolve) => setTimeout(resolve, 0));
const defaults = (): SettingsDto => ({
  scale: "Large",
  companyName: null,
  bossName: null,
  manualPaths: [],
  sound: null,
  osNotifications: null,
  rawDiagnostics: null,
});
const button = (text: string) =>
  [...document.querySelectorAll<HTMLButtonElement>("button")].find(
    (el) => el.textContent === text || el.getAttribute("aria-label") === text,
  )!;
const toggle = (name: string) =>
  document.querySelector<HTMLButtonElement>(`[role="switch"][aria-label="${name}"]`)!;
const errorText = () => document.querySelector<HTMLElement>('[role="alert"]')!.textContent;

function setup(over: Partial<SettingsDto> = {}) {
  let settings = { ...defaults(), ...over };
  const invoke = vi.fn(async (cmd: string, args?: Record<string, unknown>) => {
    if (cmd === "get_settings") return structuredClone(settings);
    if (cmd === "set_settings") {
      settings = { ...settings, ...(args?.dto as Partial<SettingsDto>) };
      return;
    }
    if (cmd === "get_diagnostics") return "비식별 진단 <home>";
  });
  const onSettings = vi.fn();
  const writeClipboard = vi.fn().mockResolvedValue(undefined);
  openSettings({ invoke: invoke as Invoke, onSettings, writeClipboard });
  return { invoke, onSettings, writeClipboard, settings: () => settings };
}

afterEach(() => {
  document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
  document.body.innerHTML = "";
});

test("불러온 설정과 기본값을 표시하고 원문 진단 설명은 항상 보인다", async () => {
  const s = setup({ companyName: "야근상사", bossName: "김사장" });
  await settle();
  expect(document.querySelector<HTMLInputElement>("#settings-companyName")!.value).toBe("야근상사");
  expect(document.querySelector<HTMLInputElement>("#settings-bossName")!.value).toBe("김사장");
  expect(toggle("소리").getAttribute("aria-checked")).toBe("false");
  expect(toggle("OS 알림").getAttribute("aria-checked")).toBe("true");
  expect(toggle("원문 진단").getAttribute("aria-checked")).toBe("false");
  expect(document.body.textContent).toContain("명령 원문 80자와 결과 앞 200자");
  expect(document.body.textContent).not.toContain("컨텍스트 창 기본값");
  expect(s.invoke).toHaveBeenCalledExactlyOnceWith("get_settings");
});

test("소리 제스처는 저장을 기다리지 않고 켜기·끄기 클릭에서 전달된다", async () => {
  let settings = defaults();
  const gesture = vi.fn();
  const invoke: Invoke = async <T>(cmd: string, args?: Record<string, unknown>) => {
    if (cmd === "set_settings") {
      const dto = args?.dto as Partial<SettingsDto>;
      expect(gesture).toHaveBeenLastCalledWith(dto.sound);
      settings = { ...settings, ...dto };
    }
    return settings as T;
  };
  openSettings({ invoke, onSettings() {}, onSoundGesture: gesture });
  await settle();
  expect(gesture).not.toHaveBeenCalled();
  toggle("소리").click();
  expect(gesture).toHaveBeenLastCalledWith(true);
  await settle();
  toggle("소리").click();
  expect(gesture).toHaveBeenLastCalledWith(false);
});

test("변경한 필드만 저장하고 다시 읽은 값으로 UI와 호출자를 갱신한다", async () => {
  const s = setup();
  await settle();
  toggle("소리").click();
  expect(document.querySelector("fieldset")!.disabled).toBe(true);
  await settle();
  expect(s.invoke).toHaveBeenCalledWith("set_settings", { dto: { sound: true } });
  expect(s.invoke.mock.calls.filter(([cmd]) => cmd === "get_settings")).toHaveLength(2);
  expect(toggle("소리").getAttribute("aria-checked")).toBe("true");
  expect(s.onSettings).toHaveBeenLastCalledWith(expect.objectContaining({ sound: true }));
  button("중소기업").click();
  await settle();
  expect(s.invoke).toHaveBeenCalledWith("set_settings", { dto: { scale: "Small" } });
  expect(button("중소기업").getAttribute("aria-pressed")).toBe("true");
});

test("저장 실패여도 바뀐 백엔드 메모리를 다시 읽고 오류를 숨기지 않는다", async () => {
  let settings = defaults();
  const invoke = vi.fn(async (cmd: string, args?: Record<string, unknown>) => {
    if (cmd === "set_settings") {
      settings = { ...settings, ...(args?.dto as Partial<SettingsDto>) };
      throw new Error("디스크 잠김");
    }
    return settings;
  });
  const onSettings = vi.fn();
  openSettings({ invoke: invoke as Invoke, onSettings });
  await settle();
  toggle("원문 진단").click();
  await settle();
  expect(toggle("원문 진단").getAttribute("aria-checked")).toBe("true");
  expect(onSettings).toHaveBeenLastCalledWith(expect.objectContaining({ rawDiagnostics: true }));
  expect(errorText()).toContain("디스크 잠김");
  expect(document.querySelector<HTMLElement>('[role="alert"]')!.hidden).toBe(false);
});

test("저장 뒤 재조회 실패도 확인 실패로 알리고 성공을 가장하지 않는다", async () => {
  const invoke = vi
    .fn()
    .mockResolvedValueOnce(undefined)
    .mockRejectedValueOnce(new Error("연결 끊김"));
  const onSettings = vi.fn();
  await expect(saveSettings({ invoke, onSettings }, { sound: true })).rejects.toThrow(
    "현재 설정을 확인하지 못했습니다",
  );
  expect(onSettings).not.toHaveBeenCalled();
});

test("이름은 Unicode 20자를 허용하고 초과는 저장하지 않으며 빈 값으로 기본 복원한다", async () => {
  const s = setup();
  await settle();
  const input = document.querySelector<HTMLInputElement>("#settings-companyName")!;
  input.value = "🦉".repeat(20);
  input.dispatchEvent(new Event("change"));
  await settle();
  expect(s.settings().companyName).toBe("🦉".repeat(20));
  const calls = s.invoke.mock.calls.length;
  input.value = "🦉".repeat(21);
  input.dispatchEvent(new Event("change"));
  await settle();
  expect(s.invoke.mock.calls).toHaveLength(calls);
  expect(errorText()).toContain("20자");
  input.value = "  ";
  input.dispatchEvent(new Event("change"));
  await settle();
  expect(s.invoke).toHaveBeenCalledWith("set_settings", { dto: { companyName: "" } });
});

test("제품과 세션 루트 자체로 경로를 추가·삭제하고 상대 경로는 거절한다", async () => {
  const s = setup({ manualPaths: [{ vendor: "Claude", root: "C:\\samples\\projects" }] });
  await settle();
  const input = document.querySelector<HTMLInputElement>("#settings-path")!;
  input.value = "relative";
  button("폴더 직접 지정").click();
  expect(errorText()).toContain("절대 경로");
  expect(s.invoke.mock.calls.filter(([cmd]) => cmd === "set_settings")).toHaveLength(0);
  input.value = "\\\\wsl.localhost\\Sample\\home\\user\\sessions";
  document.querySelector<HTMLSelectElement>('select[aria-label="세션 제품"]')!.value = "Codex";
  button("폴더 직접 지정").click();
  await settle();
  expect(s.settings().manualPaths).toEqual([
    { vendor: "Claude", root: "C:\\samples\\projects" },
    { vendor: "Codex", root: input.value },
  ]);
  button("이 경로 빼기").click();
  await settle();
  expect(s.settings().manualPaths).toEqual([{ vendor: "Codex", root: input.value }]);
});

test("진단은 요청한 때만 복사하고 클립보드 실패를 화면에 보인다", async () => {
  const s = setup();
  await settle();
  expect(s.writeClipboard).not.toHaveBeenCalled();
  s.writeClipboard.mockRejectedValueOnce(new Error("클립보드 거부"));
  button("진단 정보 복사").click();
  await settle();
  expect(errorText()).toContain("클립보드 거부");
  button("진단 정보 복사").click();
  await settle();
  expect(s.writeClipboard).toHaveBeenLastCalledWith("비식별 진단 <home>");
  expect(document.querySelector('[role="status"]')!.textContent).toContain("복사했습니다");
  button("미대응 로그 폴더 열기").click();
  await settle();
  expect(s.invoke).toHaveBeenCalledWith("open_log_folder");
});

test("초기 읽기가 실패하면 수정은 막고 다시 불러올 수 있다", async () => {
  const invoke = vi
    .fn()
    .mockRejectedValueOnce(new Error("일시 실패"))
    .mockResolvedValue(defaults());
  openSettings({ invoke, onSettings: vi.fn() });
  await settle();
  expect(errorText()).toContain("일시 실패");
  expect(document.querySelector("fieldset")!.disabled).toBe(true);
  button("다시 불러오기").click();
  await settle();
  expect(document.querySelector("fieldset")!.disabled).toBe(false);
});

test("저장 중 팝업을 닫아도 메모리 반영과 저장 실패 알림은 호출자에게 전달한다", async () => {
  let reject!: (reason: Error) => void;
  const pending = new Promise((_, no) => {
    reject = no;
  });
  const changed = { ...defaults(), sound: true };
  const invoke = vi
    .fn()
    .mockResolvedValueOnce(defaults())
    .mockReturnValueOnce(pending)
    .mockResolvedValueOnce(changed);
  const onSettings = vi.fn();
  const onError = vi.fn();
  openSettings({ invoke, onSettings, onError });
  await settle();
  toggle("소리").click();
  document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
  reject(new Error("디스크 잠김"));
  await settle();
  expect(onSettings).toHaveBeenLastCalledWith(changed);
  expect(onError).toHaveBeenCalledWith(expect.stringContaining("디스크 잠김"));
});

test("공통 여백 변경은 설정 머리와 본문에 20px을 적용하고 저장 버튼 높이는 보존한다", async () => {
  const { readFileSync } = await vi.importActual<{
    readFileSync: (path: string, encoding: "utf8") => string;
  }>("node:fs");
  const style = document.createElement("style");
  style.textContent =
    readFileSync("design/handoff/tokens.css", "utf8") +
    readFileSync("src/styles/popup.css", "utf8");
  document.head.append(style);
  try {
    setup();
    await settle();
    const css = (selector: string) => getComputedStyle(document.querySelector(selector)!);
    expect(css(".ow-pop__head").paddingLeft).toBe("20px");
    expect(css(".ow-settings").paddingLeft).toBe("20px");
    expect(css(".ow-settings").paddingBottom).toBe("20px");
    expect(css(".ow-modal__foot .ow-btn").height).toBe("36px");
  } finally {
    style.remove();
  }
});
