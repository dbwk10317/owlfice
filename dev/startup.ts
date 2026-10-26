import "../design/handoff/tokens.css";
import "../src/styles/app.css";
import ui from "../design/sprites/ui.json";
import { mountEmptyOffice } from "../src/empty-office.ts";
import { showLoading, type LoadingIo, type LoadingSheet } from "../src/loading.ts";
import { openSettings } from "../src/popup-settings.ts";
import { showScale, type Invoke } from "../src/scale.ts";
import { createStore } from "../src/store.ts";
import type {
  DiscoveryPathDto,
  OfficeDto,
  OfficeListDto,
  SettingsDto,
  StartupDetail,
  StartupProgressDto,
  StartupSummary,
} from "../src/types/ipc.ts";

const stage = document.getElementById("stage")!;
const status = document.getElementById("status")!;
const controls = document.getElementById("controls")!;
const next = document.getElementById("next") as HTMLButtonElement;
const sheet: LoadingSheet = {
  frames: ui.frames,
  size: ui.meta.size,
  building: {
    windows: ui.owlfice.building.windows.map(([x, y]) => [x, y]),
    lightOrder: ui.owlfice.building.lightOrder,
    signOffset: [ui.owlfice.building.signOffset[0], ui.owlfice.building.signOffset[1]],
  },
};
const mode = new URLSearchParams(location.search).get("mode") ?? "first";
const history: StartupProgressDto[] = [];
let handler: Parameters<LoadingIo["listen"]>[1] | undefined;
let step = 1;
let companyName: string | undefined;
let settings: SettingsDto = {
  scale: "Large",
  companyName: null,
  manualPaths: [],
  sound: false,
  nightMode: "auto",
  osNotifications: true,
  bossName: null,
  rawDiagnostics: false,
  animal: null,
};
const candidates: DiscoveryPathDto[] = [
  { host: "Windows", vendor: "Claude", path: "C:\\Preview\\.claude\\projects", status: "Missing" },
  { host: "WSL Preview-Linux", vendor: "Codex", path: null, status: "Offline" },
  {
    host: "WSL Unknown-Linux",
    vendor: "Claude",
    path: "\\\\wsl.localhost\\Unknown-Linux\\home\\preview\\.claude\\projects",
    status: "Unknown",
  },
];
let discovery: DiscoveryPathDto[] = candidates;
let discoveryFails = false;

// loading.test.ts와 같은 완전한 진행 이벤트를 사용한다.
function ev(
  step: number,
  state: StartupProgressDto["state"],
  summary: StartupSummary = { kind: "None" },
  detail: StartupDetail | null = null,
): StartupProgressDto {
  return { step, state, summary, detail };
}
const summaries: StartupSummary[] = [
  { kind: "None" },
  { kind: "Candidates", n: 2, hosts: ["Windows"] },
  { kind: "Rented", buildings: ["Windows Claude", "Windows Codex"] },
  { kind: "Teams", n: 4 },
  { kind: "Hired", n: 12, bujang: 4, gwajang: 2, daeri: 3, sawon: 3 },
  { kind: "None" },
  { kind: "None" },
];
const invoke: Invoke = async <T>(cmd: string, args?: Record<string, unknown>): Promise<T> => {
  let result: unknown;
  if (cmd === "get_startup_progress") result = [...history];
  else if (cmd === "get_discovery") {
    if (discoveryFails) throw new Error("합성 탐색 정보 읽기 실패");
    result = discovery;
  } else if (cmd === "get_settings") result = settings;
  else if (cmd === "set_settings")
    settings = { ...settings, ...(args?.dto as Partial<SettingsDto>) };
  else if (cmd === "get_diagnostics") result = "합성 진단 · 실제 경로를 읽지 않았습니다";
  else if (cmd !== "rescan" && cmd !== "open_log_folder")
    throw new Error(`합성 응답이 없는 명령: ${cmd}`);
  return result as T;
};
const io: LoadingIo = {
  invoke,
  listen: async (event, receive) => {
    if (event !== "startup-progress") throw new Error(`합성 이벤트 없음: ${event}`);
    handler = receive;
    return () => {
      handler = undefined;
    };
  },
  random: () => 0,
};
function send(event: StartupProgressDto): void {
  history.push(event);
  handler?.({ payload: event });
}
function empty(): void {
  stage.replaceChildren();
  const app = stage.appendChild(document.createElement("div"));
  app.className = "ow-app";
  const host = app.appendChild(document.createElement("div"));
  host.className = "ow-stage";
  const store = createStore<OfficeListDto, OfficeDto>();
  store.applyWorld({
    listSeq: 1,
    rebuilt: false,
    scale: "Large",
    offices: [],
    selectedId: null,
    selectRequest: 0,
    selected: null,
  });
  mountEmptyOffice(host, store, {
    invoke,
    sheet,
    onSettings: (opener) => openSettings({ invoke, onSettings: () => {} }, opener),
  });
  const label = document.createElement("label");
  label.textContent = "탐색 결과 ";
  const select = document.createElement("select");
  for (const [value, text] of [
    ["mixed", "없음·소등·미확인"],
    ["found", "모두 퇴근 (Found)"],
    ["missing", "폴더 없음 (Missing)"],
    ["offline", "배포판 소등 (Offline)"],
    ["unknown", "미확인 (Unknown)"],
    ["pending", "정보 준비 중"],
    ["error", "조회 실패"],
  ]) {
    const option = document.createElement("option");
    option.value = value;
    option.textContent = text;
    select.append(option);
  }
  select.addEventListener("change", () => {
    discoveryFails = select.value === "error";
    discovery =
      select.value === "found"
        ? [{ ...candidates[0], status: "Found" }]
        : select.value === "pending"
          ? []
          : select.value === "mixed" || discoveryFails
            ? candidates
            : candidates.filter((path) => path.status.toLowerCase() === select.value);
    status.textContent = "합성 응답 변경 · 실제 3초 캐시 조회 뒤 반영";
  });
  label.append(select);
  controls.append(label);
  status.textContent = "실제 빈 상태 컴포넌트 · 합성 경로만 표시";
  next.disabled = true;
}
function loading(failure = false): void {
  step = failure ? 4 : 1;
  history.length = 0;
  if (failure)
    history.push(
      ev(1, "Done"),
      ev(1, "Warn", { kind: "SettingsBroken" }),
      ev(2, "Done", { kind: "Candidates", n: 0, hosts: ["Windows"] }),
      ev(3, "Error", { kind: "NoFolders" }),
      ev(3, "Warn", { kind: "WslFail", floor: 2, distro: "Preview-Linux" }),
    );
  history.push(ev(step, "Running"));
  status.textContent = failure ? "합성 실패 안내 · 미완료 단계 유지" : "합성 진행 · 1/7 단계";
  next.disabled = failure;
  void showLoading(stage, io, sheet, companyName).then(empty);
}

// 조작 버튼은 제품의 '아무 키/클릭으로 건너뛰기'에 섞이지 않게 한다.
controls.addEventListener("click", (event) => event.stopPropagation());
controls.addEventListener("keydown", (event) => event.stopPropagation());
for (const button of controls.querySelectorAll<HTMLButtonElement>("button[data-mode]")) {
  button.setAttribute("aria-pressed", String(button.dataset.mode === mode));
  button.addEventListener("click", () => {
    location.search = new URLSearchParams({ mode: button.dataset.mode! }).toString();
  });
}
next.addEventListener("click", () => {
  if (step > 7) return;
  send(ev(step, "Done", summaries[step - 1]));
  step++;
  if (step <= 7) {
    send(
      ev(
        step,
        "Running",
        { kind: "None" },
        step === 5 ? { kind: "Files", done: 187, total: 312 } : null,
      ),
    );
    status.textContent = `합성 진행 · ${step}/7 단계`;
  } else {
    next.disabled = true;
    status.textContent = "7단계 완료 · 실제 간판/페이드 뒤 빈 상태로 전환";
  }
});
if (mode === "empty") empty();
else if (mode === "progress" || mode === "failure") loading(mode === "failure");
else {
  status.textContent = "첫 실행 · 규모 선택 뒤 합성 진행으로 전환";
  void showScale(stage, invoke, sheet).then((choice) => {
    companyName = choice.companyName ?? undefined;
    loading();
  });
}
