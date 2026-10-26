import type { DownloadEvent, Update } from "@tauri-apps/plugin-updater";

type UpdateRelease = Pick<Update, "version" | "downloadAndInstall" | "close">;
export interface UpdateDeps {
  getVersion(): Promise<string>;
  check(): Promise<UpdateRelease | null>;
}
export type UpdateFailure = "check" | "manifest" | "download" | "signature" | "install";

export type UpdateState = Readonly<
  {
    currentVersion: string | null;
    versionFailed: boolean;
    downloaded: number;
    total: number | null;
  } & (
    | { phase: "idle" | "checking" | "latest"; version: null; failure: null }
    | {
        phase: "available" | "downloading" | "preparing" | "installing";
        version: string;
        failure: null;
      }
    | { phase: "failed"; version: null; failure: "check" | "manifest" }
    | { phase: "failed"; version: string; failure: "download" | "signature" | "install" }
  )
>;

/** 설정 저장·팝업 수명과 독립적으로 확인 결과와 설치 작업을 한 번만 보관한다. */
export function createUpdateStore(deps: UpdateDeps) {
  let state: UpdateState = {
    currentVersion: null,
    versionFailed: false,
    phase: "idle",
    version: null,
    downloaded: 0,
    total: null,
    failure: null,
  };
  const subscribers = new Set<() => void>();
  let release: UpdateRelease | null = null;
  let destroyed = false;
  const notify = () => subscribers.forEach((fn) => fn());
  const busy = () => ["checking", "downloading", "preparing", "installing"].includes(state.phase);
  async function close(value: UpdateRelease | null): Promise<void> {
    try {
      await value?.close();
    } catch (error) {
      console.error("업데이트 리소스 해제 실패", error);
    }
  }
  void deps.getVersion().then(
    (version) => {
      if (!destroyed) {
        state = { ...state, currentVersion: version };
        notify();
      }
    },
    () => {
      if (!destroyed) {
        state = { ...state, versionFailed: true };
        notify();
      }
    },
  );
  async function check(quiet = false): Promise<string | null> {
    if (destroyed || busy()) return null;
    state = { ...state, phase: "checking", failure: null, version: null };
    const previous = release;
    release = null;
    notify();
    await close(previous);
    if (destroyed) return null;
    try {
      const next = await deps.check();
      if (destroyed) {
        await close(next);
        return null;
      }
      release = next;
      state = next
        ? { ...state, phase: "available", version: next.version, failure: null }
        : { ...state, phase: "latest", version: null, failure: null };
      notify();
      return state.version;
    } catch (error) {
      if (!destroyed) {
        state = quiet
          ? { ...state, phase: "idle", version: null, failure: null }
          : {
              ...state,
              phase: "failed",
              version: null,
              failure: String(error).includes(
                "Could not fetch a valid release JSON from the remote",
              )
                ? "manifest"
                : "check",
            };
        notify();
      }
      return null;
    }
  }
  async function install(): Promise<void> {
    if (destroyed || busy() || !release) return;
    const installing = release;
    state = {
      ...state,
      phase: "downloading",
      version: installing.version,
      failure: null,
      downloaded: 0,
      total: null,
    };
    notify();
    let finished = false;
    const progress = (event: DownloadEvent) => {
      if (destroyed || release !== installing) return;
      if (event.event === "Started") state = { ...state, total: event.data.contentLength || null };
      else if (event.event === "Progress")
        state = { ...state, downloaded: state.downloaded + event.data.chunkLength };
      else {
        finished = true;
        state = { ...state, phase: "preparing", version: installing.version, failure: null };
      }
      notify();
    };
    try {
      // Windows는 설치기를 시작하며 종료한다. 끊긴 Promise는 대기하되 실제 reject는 숨기지 않는다.
      await installing.downloadAndInstall(progress);
      if (!destroyed)
        state = { ...state, phase: "installing", version: installing.version, failure: null };
    } catch (error) {
      if (!destroyed) {
        const message = String(error);
        state = {
          ...state,
          phase: "failed",
          version: installing.version,
          failure:
            /signature|minisign|signed for version/i.test(message) ||
            /^(?:Error: )?Invalid (?:symbol \d+, offset \d+\.|input length: \d+|last symbol \d+, offset \d+\.|padding)$/.test(
              message,
            )
              ? "signature"
              : !finished || message.includes("Download request failed with status:")
                ? "download"
                : "install",
        };
      }
    } finally {
      release = null;
      await close(installing);
      if (!destroyed) notify();
    }
  }
  return {
    get state(): UpdateState {
      return Object.freeze(state);
    },
    check,
    install,
    subscribe(fn: () => void) {
      subscribers.add(fn);
      return () => {
        subscribers.delete(fn);
      };
    },
    destroy() {
      destroyed = true;
      subscribers.clear();
      if (!["downloading", "preparing", "installing"].includes(state.phase)) {
        void close(release);
        release = null;
      }
    },
  };
}
export type UpdateStore = ReturnType<typeof createUpdateStore>;
