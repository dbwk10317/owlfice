// 빈 목록에서도 탐색 중·폴더 없음·모두 퇴근을 구분한다 (PLAN 3.9, HANDOFF 4.7).
import phrases from "../design/handoff/phrases.ko.json";
import { emptyState } from "./sidebar-left.ts";
import type { Invoke, Sheet } from "./scale.ts";
import type { createStore } from "./store.ts";
import type { DiscoveryPathDto, OfficeDto, OfficeListDto } from "./types/ipc.ts";
import "./styles/empty-office.css";

export function mountEmptyOffice(
  stage: HTMLElement,
  store: ReturnType<typeof createStore<OfficeListDto, OfficeDto>>,
  options: { invoke: Invoke; onSettings: (opener: HTMLElement) => void; sheet?: Sheet },
): { dispose(): void } {
  const doc = stage.ownerDocument;
  const root = doc.createElement("div");
  root.className = "ow-empty-office";
  root.hidden = true;
  const content = doc.createElement("div");
  content.className = "ow-empty-office__content";
  const report = doc.createElement("div");
  const pick = doc.createElement("button");
  pick.type = "button";
  pick.className = "ow-btn is-primary";
  pick.textContent = phrases.empty.pickFolder;
  pick.addEventListener("click", () => options.onSettings(pick));
  content.append(report, pick);
  root.append(content);
  stage.append(root);

  let active = false;
  let disposed = false;
  let generation = 0;
  let inFlight = false;
  let timer: ReturnType<typeof setInterval> | undefined;
  let rendered = "";

  function message(title: string, body: string, failed = false): HTMLElement {
    const state = emptyState(doc, title, body);
    report.setAttribute("role", failed ? "alert" : "status");
    report.replaceChildren(state);
    return state;
  }

  function render(paths: DiscoveryPathDto[]): void {
    const key = JSON.stringify(paths);
    if (key === rendered) return;
    rendered = key;
    pick.hidden = true;
    if (paths.length === 0) {
      message("탐색 정보를 준비하고 있습니다", "찾아본 경로 정보를 불러오는 중입니다.");
      return;
    }
    if (paths.some((path) => path.status === "Found")) {
      const state = message(phrases.empty.allOffTitle, phrases.empty.allOffBody);
      state.querySelector(".ow-empty__art")?.remove();
      const sheet = options.sheet;
      const frame = sheet?.frames["building/base"]?.frame;
      if (sheet && frame) {
        const building = doc.createElement("div");
        building.className = "ow-empty-office__building";
        building.setAttribute("aria-hidden", "true");
        building.style.cssText =
          `width:${frame.w * 2}px;height:${frame.h * 2}px;background:url(/ui.png) ` +
          `-${frame.x * 2}px -${frame.y * 2}px/${sheet.size.w * 2}px ${sheet.size.h * 2}px no-repeat`;
        state.prepend(building);
      }
      return;
    }
    message(phrases.empty.noFoldersTitle, phrases.empty.noFoldersBody);
    pick.hidden = false;
    const table = doc.createElement("table");
    table.className = "ow-empty-office__paths";
    const caption = table.createCaption();
    caption.textContent = phrases.empty.searchedPaths;
    const body = table.createTBody();
    for (const path of paths) {
      const row = body.insertRow();
      const host = doc.createElement("th");
      host.scope = "row";
      const badge = doc.createElement("span");
      badge.className = "ow-host";
      badge.textContent = path.host;
      const vendor = doc.createElement("span");
      vendor.textContent = path.vendor;
      host.append(badge, vendor);
      row.append(host);
      const value = row.insertCell();
      value.className = "ow-empty-office__path ow-mono";
      value.textContent = path.path ?? "경로 미확인";
      const result = row.insertCell();
      result.textContent =
        path.status === "Missing"
          ? phrases.empty.pathMissing
          : path.status === "Offline"
            ? phrases.empty.pathOff
            : "확인 못함";
    }
    report.append(table);
    if (paths.some((path) => path.status === "Offline")) {
      const explanation = doc.createElement("p");
      explanation.textContent = phrases.empty.pathOffBody;
      report.append(explanation);
    }
  }

  async function refresh(): Promise<void> {
    if (disposed || !active || doc.hidden || inFlight) return;
    inFlight = true;
    const request = generation;
    try {
      const paths = await options.invoke<DiscoveryPathDto[]>("get_discovery");
      if (!disposed && active && !doc.hidden && request === generation) render(paths);
    } catch (error) {
      if (!disposed && active && !doc.hidden && request === generation) {
        console.error("get_discovery", error);
        if (rendered !== "failed") {
          rendered = "failed";
          message(
            "찾아본 경로를 불러오지 못했습니다",
            "잠시 후 다시 확인합니다. 설정에서 경로를 직접 지정할 수도 있습니다.",
            true,
          );
          pick.hidden = false;
        }
      }
    } finally {
      inFlight = false;
      // 숨김·새 목록 이전 응답은 버리고, 다시 빈 화면이 됐다면 현재 캐시만 읽는다.
      if (!disposed && active && !doc.hidden && request !== generation) void refresh();
    }
  }

  function update(): void {
    const next = store.state.list !== null && store.state.list.offices.length === 0;
    if (next !== active) {
      active = next;
      generation++;
      if (active) {
        rendered = "";
        render([]);
      }
    }
    root.hidden = !active;
    if (!active || doc.hidden) {
      if (timer !== undefined) {
        clearInterval(timer);
        timer = undefined;
        generation++;
      }
    } else if (timer === undefined) {
      generation++;
      timer = setInterval(() => void refresh(), 3000);
      void refresh();
    }
  }

  const unsubscribe = store.subscribe(update);
  doc.addEventListener("visibilitychange", update);
  update();
  return {
    dispose() {
      disposed = true;
      generation++;
      clearInterval(timer);
      unsubscribe();
      doc.removeEventListener("visibilitychange", update);
      root.remove();
    },
  };
}
