// @vitest-environment jsdom
import { afterEach, expect, test, vi } from "vite-plus/test";
import { openSettings, saveSettings } from "./popup-settings.ts";
import type { Invoke } from "./scale.ts";
import type { SettingsDto } from "./types/ipc.ts";
import { createUpdateStore, type UpdateDeps } from "./store.ts";
import type { DownloadEvent } from "@tauri-apps/plugin-updater";

const settle = () => new Promise((resolve) => setTimeout(resolve, 0));
const defaults = (): SettingsDto => ({
  scale: "Large",
  companyName: null,
  bossName: null,
  manualPaths: [],
  sound: null,
  nightMode: "auto",
  osNotifications: null,
  rawDiagnostics: null,
});
const button = (text: string) =>
  [...document.querySelectorAll<HTMLButtonElement>("button")].find(
    (el) => el.textContent === text || el.getAttribute("aria-label") === text,
  )!;
const toggle = (name: string) =>
  document.querySelector<HTMLButtonElement>(`[role="switch"][aria-label="${name}"]`)!;
const errorText = () =>
  document.querySelector<HTMLElement>('.ow-settings__foot [role="alert"]')!.textContent;

function setup(over: Partial<SettingsDto> = {}) {
  let settings = { ...defaults(), ...over };
  const invoke = vi.fn(async (cmd: string, args?: Record<string, unknown>) => {
    if (cmd === "get_settings") return structuredClone(settings);
    if (cmd === "set_settings") {
      settings = { ...settings, ...(args?.dto as Partial<SettingsDto>) };
      return;
    }
    if (cmd === "get_diagnostics") return "비식별 진단 <home>";
    if (cmd === "get_log_folder") return "C:\\samples\\Owlfice\\logs";
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

test.each(["auto", "on", "off"] as const)(
  "야근 모드 %s를 표시하고 세 값 모두 부분 저장한다",
  async (nightMode) => {
    const s = setup({ nightMode });
    await settle();
    const choices = { auto: "자동", on: "켜기", off: "끄기" } as const;
    for (const [mode, text] of Object.entries(choices))
      expect(button(text).getAttribute("aria-pressed")).toBe(String(mode === nightMode));
    for (const mode of ["on", "off", "auto"] as const) {
      button(choices[mode]).click();
      await settle();
      expect(s.invoke).toHaveBeenCalledWith("set_settings", { dto: { nightMode: mode } });
      expect(button(choices[mode]).getAttribute("aria-pressed")).toBe("true");
      expect(s.onSettings).toHaveBeenLastCalledWith(expect.objectContaining({ nightMode: mode }));
    }
  },
);

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
  expect(s.invoke.mock.calls.filter(([cmd]) => cmd === "get_settings")).toHaveLength(1);
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
  expect(document.querySelector<HTMLElement>('.ow-settings__foot [role="alert"]')!.hidden).toBe(
    false,
  );
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
  button("로그 폴더 열기").click();
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
    .mockResolvedValueOnce("C:\\samples\\logs")
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

test("실제 로그 경로를 HTML로 해석하지 않고 전체 선택 가능한 긴 경로로 표시한다", async () => {
  const path = `C:\\samples\\${"긴폴더".repeat(80)}\\<img src=x>\\logs`;
  const invoke = vi.fn(async (cmd: string) => (cmd === "get_log_folder" ? path : defaults()));
  openSettings({ invoke: invoke as Invoke, onSettings: vi.fn() });
  await settle();
  const displayed = document.querySelector<HTMLElement>(".ow-settings__log-path")!;
  expect(displayed.textContent).toBe(path);
  expect(displayed.querySelector("img")).toBeNull();
  expect(document.body.textContent).toContain("운영 오류 기록과 미대응 AI 동작 기록은 같은 폴더");
  const range = document.createRange();
  range.selectNodeContents(displayed);
  expect(range.toString()).toBe(path);
  expect(invoke).toHaveBeenCalledWith("get_log_folder");
});

test("로그 위치 조회 실패와 지연은 설정 저장을 막지 않고 실패는 저장 후에도 보인다", async () => {
  let reject!: (reason: Error) => void;
  const pending = new Promise<string>((_, no) => {
    reject = no;
  });
  let settings = defaults();
  const invoke = vi.fn(async (cmd: string, args?: Record<string, unknown>) => {
    if (cmd === "get_log_folder") return pending;
    if (cmd === "set_settings") settings = { ...settings, ...(args?.dto as Partial<SettingsDto>) };
    return settings;
  });
  openSettings({ invoke: invoke as Invoke, onSettings: vi.fn() });
  await settle();
  expect(document.querySelector("fieldset")!.disabled).toBe(false);
  toggle("소리").click();
  await settle();
  expect(settings.sound).toBe(true);
  reject(new Error("로그 위치 접근 실패"));
  await settle();
  toggle("원문 진단").click();
  await settle();
  expect(settings.rawDiagnostics).toBe(true);
  expect(document.querySelector(".ow-settings__log-path")!.textContent).toBe("");
  expect(document.body.textContent).toContain(
    "로그 위치를 불러오지 못했습니다: Error: 로그 위치 접근 실패",
  );
  expect(document.querySelector<HTMLElement>("#settings-log-error")!.hidden).toBe(false);
});

test.each([false, true])(
  "닫힌 설정은 늦은 로그 경로 응답과 실패를 무시한다 (%s)",
  async (fails) => {
    let resolve!: (path: string) => void;
    let reject!: (reason: Error) => void;
    const pending = new Promise<string>((yes, no) => {
      resolve = yes;
      reject = no;
    });
    const invoke = vi.fn(async (cmd: string) => (cmd === "get_log_folder" ? pending : defaults()));
    const onError = vi.fn();
    openSettings({ invoke: invoke as Invoke, onSettings: vi.fn(), onError });
    await settle();
    const displayed = document.querySelector<HTMLElement>(".ow-settings__log-path")!;
    const before = displayed.textContent;
    document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
    if (fails) reject(new Error("늦은 조회 실패"));
    else resolve("C:\\samples\\late-logs");
    await settle();
    expect(displayed.textContent).toBe(before);
    expect(document.querySelector('[role="dialog"]')).toBeNull();
    expect(onError).not.toHaveBeenCalled();
  },
);

test("로그 폴더 열기는 경로 인자 없이 요청하고 탐색기 실행 실패를 알린다", async () => {
  const s = setup();
  await settle();
  s.invoke.mockRejectedValueOnce(new Error("탐색기 실행 실패"));
  button("로그 폴더 열기").click();
  await settle();
  expect(s.invoke.mock.calls.filter(([cmd]) => cmd === "open_log_folder")).toEqual([
    ["open_log_folder"],
  ]);
  expect(errorText()).toContain("탐색기 실행 실패");
  expect(document.querySelector("fieldset")!.disabled).toBe(false);
});

test("공통 여백과 로그 긴 경로 줄바꿈·선택을 적용하고 저장 버튼 높이는 보존한다", async () => {
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
    expect(css(".ow-settings__log-path").overflowWrap).toBe("anywhere");
    expect(css(".ow-settings__log-path").userSelect).toBe("text");
    expect(css(".ow-settings__log-path").minWidth).toBe("0px");
  } finally {
    style.remove();
  }
});

function updateSetup(check: UpdateDeps["check"]) {
  const updates = createUpdateStore({ getVersion: async () => "0.1.1", check });
  const invoke: Invoke = async <T>(cmd: string) =>
    (cmd === "get_settings" ? defaults() : "C:\\samples\\logs") as T;
  const open = () => openSettings({ invoke, onSettings() {}, updates });
  open();
  return { updates, open };
}
const updateText = () =>
  [...document.querySelectorAll(".ow-settings__section")].find(
    (el) => el.querySelector("h2")?.textContent === "업데이트",
  )!.textContent;

test("현재 버전과 확인 중·최신을 표시하고 중복 확인은 시작하지 않는다", async () => {
  let resolve!: (value: null) => void;
  const check = vi.fn(
    () =>
      new Promise<null>((done) => {
        resolve = done;
      }),
  );
  const { updates } = updateSetup(check);
  await settle();
  expect(updateText()).toContain("현재 버전 0.1.1");
  button("새 버전 확인").click();
  expect(updateText()).toContain("확인하는 중이에요");
  expect(button("새 버전 확인").disabled).toBe(true);
  void updates.check();
  expect(check).not.toHaveBeenCalled();
  await settle();
  expect(check).toHaveBeenCalledTimes(1);
  resolve(null);
  await settle();
  expect(updateText()).toContain("최신 버전이에요");
  expect(button("새 버전 확인").disabled).toBe(false);
  updates.destroy();
});

test("새 버전은 명시 설치만 시작하며 닫기·재열기·다른 설정 저장 뒤 진행을 유지한다", async () => {
  let progress!: (event: DownloadEvent) => void;
  let finish!: () => void;
  const downloadAndInstall = vi.fn((callback?: (event: DownloadEvent) => void) => {
    progress = callback!;
    return new Promise<void>((resolve) => {
      finish = resolve;
    });
  });
  const close = vi.fn().mockResolvedValue(undefined);
  const { updates, open } = updateSetup(async () => ({
    version: "0.1.2",
    downloadAndInstall,
    close,
  }));
  await settle();
  button("새 버전 확인").click();
  await settle();
  expect(updateText()).toContain("새 버전 0.1.2을 설치할 수 있어요");
  expect(downloadAndInstall).not.toHaveBeenCalled();
  toggle("소리").click();
  await settle();
  expect(button("0.1.2 설치").hidden).toBe(false);
  button("0.1.2 설치").click();
  void updates.install();
  expect(downloadAndInstall).toHaveBeenCalledTimes(1);
  progress({ event: "Started", data: { contentLength: 200 } });
  progress({ event: "Progress", data: { chunkLength: 50 } });
  expect(updateText()).toContain("25%");
  document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
  progress({ event: "Progress", data: { chunkLength: 50 } });
  open();
  await settle();
  expect(updateText()).toContain("50%");
  expect(button("새 버전 확인").disabled).toBe(true);
  void updates.check();
  void updates.install();
  expect(downloadAndInstall).toHaveBeenCalledTimes(1);
  progress({ event: "Finished" });
  expect(updateText()).toContain("서명을 확인하고 설치를 준비");
  expect(close).not.toHaveBeenCalled();
  finish();
  await settle();
  expect(updateText()).toContain("설치를 시작했어요");
  expect(close).toHaveBeenCalledTimes(1);
  progress({ event: "Progress", data: { chunkLength: 100 } });
  progress({ event: "Finished" });
  expect(updateText()).toContain("설치를 시작했어요");
  updates.destroy();
});

test("다운로드 크기를 모르면 받은 바이트를 표시한다", async () => {
  let progress!: (event: DownloadEvent) => void;
  let reject!: (error: string) => void;
  const { updates } = updateSetup(async () => ({
    version: "0.1.2",
    close: async () => {},
    downloadAndInstall(callback) {
      progress = callback!;
      return new Promise<void>((_, fail) => {
        reject = fail;
      });
    },
  }));
  await settle();
  button("새 버전 확인").click();
  await settle();
  button("0.1.2 설치").click();
  progress({ event: "Started", data: {} });
  progress({ event: "Progress", data: { chunkLength: 512 } });
  expect(updateText()).toContain("512바이트");
  reject("offline");
  await settle();
  expect(updateText()).toContain("다운로드하지 못했어요");
  progress({ event: "Finished" });
  expect(updateText()).toContain("다운로드하지 못했어요");
  updates.destroy();
});

test.each([
  ["Could not fetch a valid release JSON from the remote", "안내 파일을 찾지 못했어요"],
  ["error sending request for url", "새 버전을 확인하지 못했어요"],
])("확인 String 오류를 최신으로 오인하지 않는다: %s", async (failure, expected) => {
  const { updates } = updateSetup(async () => {
    throw failure;
  });
  await settle();
  button("새 버전 확인").click();
  await settle();
  expect(updateText()).toContain(expected);
  expect(updateText()).not.toContain("최신 버전이에요");
  updates.destroy();
});

test.each([
  ["Download request failed with status: 404 Not Found", false, "다운로드하지 못했어요"],
  ["The signature verification failed", true, "서명을 확인하지 못했어요"],
  ["Invalid encoding in minisign data", true, "서명을 확인하지 못했어요"],
  ["Invalid symbol 35, offset 100.", true, "서명을 확인하지 못했어요"],
  ["Invalid input length: 101", true, "서명을 확인하지 못했어요"],
  ["Invalid last symbol 65, offset 100.", true, "서명을 확인하지 못했어요"],
  ["Invalid padding", true, "서명을 확인하지 못했어요"],
  ["Unexpected signature algorithm", true, "서명을 확인하지 못했어요"],
  [
    "The signature was created with a different key than the one provided",
    true,
    "서명을 확인하지 못했어요",
  ],
  [
    "The update was signed for version 0.1.1 but the update endpoint announced version 0.1.2.",
    true,
    "서명을 확인하지 못했어요",
  ],
  [
    "The update signature does not specify the version it was signed for",
    true,
    "서명을 확인하지 못했어요",
  ],
  ["failed to create temporary directory", true, "설치를 시작하지 못했어요"],
])("Finished 뒤에도 실제 설치 오류를 표시한다: %s", async (failure, finished, expected) => {
  const close = vi.fn().mockResolvedValue(undefined);
  const { updates } = updateSetup(async () => ({
    version: "0.1.2",
    close,
    async downloadAndInstall(progress) {
      if (finished) progress?.({ event: "Finished" });
      throw failure;
    },
  }));
  await settle();
  button("새 버전 확인").click();
  await settle();
  button("0.1.2 설치").click();
  await settle();
  expect(updateText()).toContain(expected);
  expect(button("새 버전 확인").disabled).toBe(false);
  expect(close).toHaveBeenCalledTimes(1);
  updates.destroy();
});
