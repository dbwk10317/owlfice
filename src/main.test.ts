// @vitest-environment jsdom
import { afterEach, beforeEach, expect, test, vi } from "vite-plus/test";
import type { SettingsDto } from "./types/ipc.ts";

const mocked = vi.hoisted(() => ({
  invoke: vi.fn(),
  listen: vi.fn(),
  mountSidebar: vi.fn(),
}));

vi.mock("@tauri-apps/api/core", () => ({ invoke: mocked.invoke }));
vi.mock("@tauri-apps/api/event", () => ({ listen: mocked.listen }));
vi.mock("@tauri-apps/api/app", () => ({ getVersion: async () => "0.2.3" }));
vi.mock("@tauri-apps/plugin-updater", () => ({ check: async () => null }));
vi.mock("./loading.ts", () => ({ showLoading: async () => {} }));
vi.mock("./office/renderer.ts", () => ({ mountOffice: vi.fn() }));
vi.mock("./sidebar-left.ts", () => ({
  mountSidebar: mocked.mountSidebar,
  emptyState: (doc: Document, title: string, body: string) => {
    const el = doc.createElement("div");
    el.textContent = `${title} ${body}`;
    return el;
  },
  popupNames: vi.fn(),
}));

const settings: SettingsDto = {
  scale: "Large",
  companyName: null,
  bossName: null,
  manualPaths: [],
  sound: null,
  nightMode: "auto",
  osNotifications: null,
  rawDiagnostics: null,
  animal: null,
};

beforeEach(() => {
  vi.resetModules();
  mocked.invoke.mockReset();
  mocked.listen.mockReset();
  mocked.mountSidebar.mockReset();
  vi.stubGlobal(
    "fetch",
    vi.fn(async () => ({ json: async () => ({ frames: {}, meta: {} }) })),
  );
});

afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
  document.body.replaceChildren();
});

async function run(): Promise<void> {
  vi.spyOn(console, "error").mockImplementation(() => {});
  await import("./main.ts");
  await vi.waitFor(() => expect(document.querySelector(".ow-start-error")).not.toBeNull());
}

test("한 이벤트 구독만 성공하면 해제하고 초기 사무실을 만들지 않는다", async () => {
  const stop = vi.fn(async () => {});
  mocked.invoke.mockImplementation(async (cmd: string) =>
    cmd === "get_settings" ? settings : undefined,
  );
  mocked.listen.mockImplementation((event: string) =>
    event === "office-list" ? Promise.resolve(stop) : Promise.reject(new Error("구독 실패")),
  );
  await run();
  expect(stop).toHaveBeenCalledOnce();
  expect(mocked.invoke).not.toHaveBeenCalledWith("get_world_snapshot");
  expect(mocked.mountSidebar).not.toHaveBeenCalled();
});

test("초기 스냅샷 실패 시 두 이벤트를 해제하고 초기 사무실을 만들지 않는다", async () => {
  const stopList = vi.fn(async () => {});
  const stopState = vi.fn(async () => {});
  mocked.invoke.mockImplementation(async (cmd: string) => {
    if (cmd === "get_settings") return settings;
    if (cmd === "get_world_snapshot") throw new Error("스냅샷 실패");
  });
  mocked.listen.mockImplementation((event: string) =>
    Promise.resolve(event === "office-list" ? stopList : stopState),
  );
  await run();
  expect(stopList).toHaveBeenCalledOnce();
  expect(stopState).toHaveBeenCalledOnce();
  expect(mocked.mountSidebar).not.toHaveBeenCalled();
});
