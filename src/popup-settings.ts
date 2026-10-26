import phrases from "../design/handoff/phrases.ko.json";
import { openModal } from "./popup-modal.ts";
import { h } from "./sidebar-right.ts";
import type { Invoke } from "./scale.ts";
import type { Animal, NightMode, SettingsDto } from "./types/ipc.ts";
import type { UpdateStore } from "./store.ts";

const S = phrases.settings;

export interface SettingsDeps {
  invoke: Invoke;
  onSettings(settings: SettingsDto): void;
  onSoundGesture?(on: boolean): void;
  onError?(message: string): void;
  doc?: Document;
  writeClipboard?(text: string): Promise<void>;
  updates?: UpdateStore;
}

/** 디스크 저장 실패 때도 백엔드 메모리는 바뀌므로 다시 읽은 값으로 화면을 맞춘다. */
export async function saveSettings(deps: SettingsDeps, dto: Partial<SettingsDto>): Promise<void> {
  let failure = "";
  try {
    await deps.invoke("set_settings", { dto });
  } catch (error) {
    failure = `설정을 파일에 저장하지 못했습니다: ${String(error)}`;
  }
  try {
    deps.onSettings(await deps.invoke<SettingsDto>("get_settings"));
  } catch (error) {
    failure += `${failure ? " " : ""}현재 설정을 확인하지 못했습니다: ${String(error)}`;
  }
  if (failure) throw new Error(failure);
}

export function openSettings(deps: SettingsDeps, opener?: HTMLElement | null): void {
  const doc = deps.doc ?? document;
  const modal = openModal(doc, { width: 580, titleId: "settings-title", opener });
  const title = h(doc, "h1", "ow-pop__title", S.title);
  title.id = "settings-title";
  const head = h(
    doc,
    "header",
    "ow-pop__head",
    h(doc, "div", "ow-pop__headmain", title),
    modal.closeBtn,
  );
  const body = doc.createElement("fieldset");
  body.className = "ow-settings";
  const scroll = h(doc, "div", "ow-modal__body", body);
  const error = h(doc, "p", "ow-pop__error");
  error.setAttribute("role", "alert");
  error.hidden = true;
  const status = h(doc, "p", "ow-settings__status", "설정을 불러오는 중입니다.");
  status.setAttribute("role", "status");
  const retry = button("다시 불러오기", () => void load());
  retry.hidden = true;
  const foot = h(doc, "footer", "ow-modal__foot ow-settings__foot", error, status, retry);
  modal.modal.append(head, scroll, foot);
  let current: SettingsDto | null = null;
  let busy = false;

  function button(text: string, action: () => void): HTMLButtonElement {
    const el = doc.createElement("button");
    el.type = "button";
    el.className = "ow-btn";
    el.textContent = text;
    el.addEventListener("click", action);
    return el;
  }
  function section(text: string): HTMLElement {
    const el = h(doc, "section", "ow-settings__section", h(doc, "h2", "ow-pop__h2--lg", text));
    body.append(el);
    return el;
  }
  function label(text: string, control: HTMLElement, hint?: string): HTMLElement {
    const el = h(doc, "label", "ow-settings__field", h(doc, "span", "", text), control);
    if (hint) el.append(h(doc, "small", "ow-settings__hint", hint));
    return el;
  }
  function setBusy(on: boolean): void {
    busy = on;
    body.disabled = on;
    body.setAttribute("aria-busy", String(on));
    retry.disabled = on;
  }
  function fail(message: string): void {
    error.textContent = message;
    error.hidden = false;
    status.textContent = "";
  }
  function accept(settings: SettingsDto): void {
    current = settings;
    deps.onSettings(settings);
    if (!modal.closed) render();
  }
  async function run(action: () => Promise<void>, message: string): Promise<void> {
    if (busy) return;
    setBusy(true);
    error.hidden = true;
    status.textContent = "처리 중입니다.";
    try {
      await action();
      status.textContent = message;
    } catch (e) {
      if (!modal.closed) fail(String(e));
      else deps.onError?.(String(e));
    } finally {
      if (!modal.closed) setBusy(false);
    }
  }
  function save(dto: Partial<SettingsDto>): void {
    void run(() => saveSettings({ ...deps, onSettings: accept }, dto), "저장했습니다.");
  }
  function nameInput(key: "companyName" | "bossName", placeholder: string): HTMLInputElement {
    const input = doc.createElement("input");
    input.className = "ow-input";
    input.id = `settings-${key}`;
    input.placeholder = placeholder;
    input.addEventListener("change", () => {
      if (Array.from(input.value.trim()).length > 20) {
        fail("이름은 20자까지 입력해 주세요.");
        return;
      }
      save({ [key]: input.value.trim() });
    });
    return input;
  }

  const company = section(S.company);
  const companyName = nameInput("companyName", phrases.app.brand);
  const bossName = nameInput("bossName", S.bossNamePlaceholder);
  company.append(label("회사 이름", companyName, "비워 두면 Owlfice로 표시합니다. 최대 20자."));
  const scales = h(doc, "div", "ow-settings__scale");
  scales.setAttribute("role", "group");
  scales.setAttribute("aria-label", S.scale);
  const large = button(S.large, () => save({ scale: "Large" }));
  const small = button(S.small, () => save({ scale: "Small" }));
  scales.append(large, small);
  company.append(
    h(
      doc,
      "div",
      "ow-settings__field",
      S.scale,
      scales,
      h(doc, "small", "ow-settings__hint", S.scaleHint),
    ),
  );
  company.append(label(S.bossName, bossName, `${S.bossNameHint} 최대 20자.`));
  const animals = h(doc, "div", "ow-seg");
  animals.setAttribute("role", "group");
  animals.setAttribute("aria-label", S.animal);
  const animalChoices = (["mixed", "owls", "dogs", "cats"] as const).map((animal: Animal) => {
    const control = button(S.animals[animal], () => save({ animal }));
    control.className = "";
    animals.append(control);
    return { animal, control };
  });
  company.append(
    h(
      doc,
      "div",
      "ow-settings__field",
      S.animal,
      animals,
      h(doc, "small", "ow-settings__hint", S.animalHint),
    ),
  );

  const switches = new Map<"sound" | "osNotifications" | "rawDiagnostics", HTMLButtonElement>();
  function toggle(
    parent: HTMLElement,
    key: "sound" | "osNotifications" | "rawDiagnostics",
    text: string,
    hint: string,
  ): void {
    const control = button("", () => {
      const on = !(current?.[key] ?? key === "osNotifications");
      if (key === "sound") deps.onSoundGesture?.(on);
      save({ [key]: on });
    });
    control.className = "ow-switch";
    control.setAttribute("role", "switch");
    control.setAttribute("aria-label", text);
    const hintEl = h(doc, "small", "ow-settings__hint", hint);
    hintEl.id = `settings-${key}-hint`;
    control.setAttribute("aria-describedby", hintEl.id);
    switches.set(key, control);
    parent.append(
      h(
        doc,
        "div",
        "ow-settings__toggle",
        h(doc, "div", "ow-settings__field", text, hintEl),
        control,
      ),
    );
  }
  const sound = section(S.screenSound);
  const nightModes = h(doc, "div", "ow-seg");
  nightModes.setAttribute("role", "group");
  nightModes.setAttribute("aria-label", S.nightMode);
  const nightChoices = (["auto", "on", "off"] as const).map((mode: NightMode) => {
    const control = button(S.nightModes[mode], () => save({ nightMode: mode }));
    control.className = "";
    nightModes.append(control);
    return { mode, control };
  });
  sound.append(
    h(
      doc,
      "div",
      "ow-settings__field",
      S.nightMode,
      nightModes,
      h(doc, "small", "ow-settings__hint", S.nightModeHint),
    ),
  );
  toggle(sound, "sound", S.sound, S.soundHint);
  toggle(
    sound,
    "osNotifications",
    "OS 알림",
    "창이 보이지 않을 때 30초 이상 기다린 직원의 결재 알림을 한 번 보냅니다.",
  );

  const folders = section(S.folders);
  folders.append(
    h(
      doc,
      "p",
      "ow-settings__hint",
      `${S.auto}: Windows와 켜진 WSL에서 Claude·Codex 세션 폴더를 찾습니다. 직접 지정한 폴더도 함께 확인합니다.`,
    ),
  );
  const paths = h(doc, "ul", "ow-settings__paths");
  paths.setAttribute("aria-label", S.manual);
  const vendor = doc.createElement("select");
  vendor.className = "ow-input";
  vendor.setAttribute("aria-label", "세션 제품");
  for (const name of ["Claude", "Codex"]) {
    const option = doc.createElement("option");
    option.value = option.textContent = name;
    vendor.append(option);
  }
  const path = doc.createElement("input");
  path.className = "ow-input";
  path.id = "settings-path";
  path.placeholder = "C:\\Users\\이름\\.claude\\projects";
  const add = button(S.addFolder, () => {
    const root = path.value.trim();
    if (!/^(?:[a-z]:[\\/]|[\\/]{2}(?:wsl\.localhost|wsl\$)[\\/][^\\/]+(?:[\\/]|$))/i.test(root)) {
      fail("Windows 절대 경로나 WSL 공유의 세션 폴더 경로를 입력해 주세요.");
      return;
    }
    save({
      manualPaths: [
        ...(current?.manualPaths ?? []),
        { vendor: vendor.value === "Codex" ? "Codex" : "Claude", root },
      ],
    });
  });
  folders.append(
    paths,
    label("제품", vendor),
    label(
      "세션 폴더 경로",
      path,
      "Claude는 projects, Codex는 sessions 폴더 자체를 입력합니다. 꺼진 WSL은 켜지 않습니다.",
    ),
    add,
  );

  const diag = section(S.diag);
  toggle(diag, "rawDiagnostics", S.rawDiag, S.rawDiagHint);
  const copy = button(
    S.copyDiag,
    () =>
      void run(async () => {
        const text = await deps.invoke<string>("get_diagnostics");
        if (deps.writeClipboard) await deps.writeClipboard(text);
        else {
          const clipboard = doc.defaultView?.navigator.clipboard;
          if (!clipboard) throw new Error("클립보드를 사용할 수 없습니다.");
          await clipboard.writeText(text);
        }
      }, "진단 정보를 복사했습니다."),
  );
  const logPath = h(doc, "p", "ow-settings__log-path ow-mono", S.logFolderLoading);
  logPath.setAttribute("aria-label", S.logFolder);
  const logError = h(doc, "p", "ow-pop__error");
  logError.id = "settings-log-error";
  logError.setAttribute("role", "alert");
  logError.hidden = true;
  diag.append(
    h(doc, "div", "ow-settings__field", S.logFolder, logPath),
    h(doc, "p", "ow-settings__hint", S.logFolderHint),
    logError,
  );
  const open = button(
    S.openLogFolder,
    () => void run(() => deps.invoke("open_log_folder"), "로그 폴더를 열었습니다."),
  );
  diag.append(h(doc, "div", "ow-settings__actions", copy, open));

  if (deps.updates) {
    const updates = deps.updates;
    const update = section(S.update);
    const version = h(doc, "p", "ow-settings__hint");
    const result = h(doc, "p", "ow-settings__status");
    result.setAttribute("role", "status");
    const check = button(S.checkUpdate, () => void updates.check());
    const install = button("", () => void updates.install());
    update.append(version, result, h(doc, "div", "ow-settings__actions", check, install));
    function renderUpdate(): void {
      if (modal.closed) return;
      const s = updates.state;
      version.textContent = s.currentVersion
        ? S.currentVersion.replace("{version}", s.currentVersion)
        : s.versionFailed
          ? S.versionError
          : S.versionLoading;
      check.disabled = ["checking", "downloading", "preparing", "installing"].includes(s.phase);
      install.hidden = s.phase !== "available";
      install.disabled = check.disabled;
      install.textContent = S.installUpdate.replace("{version}", s.version ?? "");
      result.textContent =
        s.phase === "failed"
          ? S.updateErrors[s.failure ?? "check"]
          : s.phase === "available"
            ? S.updateAvailable.replace("{version}", s.version ?? "")
            : s.phase === "downloading"
              ? s.total
                ? S.updateProgress.replace(
                    "{percent}",
                    String(Math.min(100, Math.floor((s.downloaded / s.total) * 100))),
                  )
                : S.updateDownloaded.replace("{bytes}", s.downloaded.toLocaleString())
              : S.updateStates[s.phase];
      result.setAttribute("role", s.phase === "failed" ? "alert" : "status");
    }
    const unsubscribe = updates.subscribe(renderUpdate);
    const close = modal.close.bind(modal);
    modal.close = (restoreFocus) => {
      unsubscribe();
      close(restoreFocus);
    };
    renderUpdate();
  }

  function render(): void {
    if (!current) return;
    companyName.value = current.companyName ?? "";
    bossName.value = current.bossName ?? "";
    large.setAttribute("aria-pressed", String(current.scale === "Large"));
    small.setAttribute("aria-pressed", String(current.scale === "Small"));
    large.classList.toggle("is-primary", current.scale === "Large");
    small.classList.toggle("is-primary", current.scale === "Small");
    for (const { mode, control } of nightChoices) {
      const selected = mode === (current.nightMode ?? "auto");
      control.setAttribute("aria-pressed", String(selected));
    }
    for (const { animal, control } of animalChoices)
      control.setAttribute("aria-pressed", String(animal === (current.animal ?? "mixed")));
    for (const [key, el] of switches)
      el.setAttribute("aria-checked", String(current[key] ?? key === "osNotifications"));
    paths.replaceChildren(
      ...(current.manualPaths ?? []).map((item, index) => {
        const remove = button(S.removePath, () =>
          save({ manualPaths: current!.manualPaths!.filter((_, i) => i !== index) }),
        );
        remove.setAttribute("aria-label", `${S.removePath}: ${item.vendor} ${item.root}`);
        return h(
          doc,
          "li",
          "ow-settings__path",
          h(doc, "span", "ow-settings__path-text", `${item.vendor} · ${item.root}`),
          remove,
        );
      }),
    );
  }
  async function load(): Promise<void> {
    setBusy(true);
    error.hidden = true;
    retry.hidden = true;
    try {
      const settings = await deps.invoke<SettingsDto>("get_settings");
      if (modal.closed) return;
      accept(settings);
      status.textContent = "바꾸면 바로 저장합니다.";
    } catch (e) {
      if (!modal.closed) {
        fail(`설정을 불러오지 못했습니다: ${String(e)}`);
        retry.hidden = false;
      }
    } finally {
      if (!modal.closed) {
        setBusy(false);
        body.disabled = current === null;
      }
    }
  }
  void load();
  void (async () => {
    try {
      const path = await deps.invoke<string>("get_log_folder");
      if (!modal.closed) logPath.textContent = path;
    } catch (e) {
      if (!modal.closed) {
        logPath.textContent = "";
        logError.textContent = `${S.logFolderError}: ${String(e)}`;
        logError.hidden = false;
      }
    }
  })();
}
