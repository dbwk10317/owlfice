import { expect, test, vi } from "vite-plus/test";
import type { DownloadEvent } from "@tauri-apps/plugin-updater";
import { createUpdateStore, type UpdateDeps } from "./update-store.ts";

test("조용한 업데이트 확인 실패는 오류를 남기지 않고 나중에 명시 확인할 수 있다", async () => {
  const check = vi.fn().mockRejectedValueOnce("offline").mockResolvedValueOnce(null);
  const updates = createUpdateStore({ getVersion: async () => "0.1.1", check });
  expect(await updates.check(true)).toBeNull();
  expect(updates.state.phase).toBe("idle");
  expect(updates.state.failure).toBeNull();
  await updates.check();
  expect(updates.state.phase).toBe("latest");
  updates.destroy();
});

test("조용한 확인은 새 버전만 반환하고 다운로드는 하지 않는다", async () => {
  const close = vi.fn().mockResolvedValue(undefined);
  const downloadAndInstall = vi.fn();
  const updates = createUpdateStore({
    getVersion: async () => {
      throw "version unavailable";
    },
    check: async () => ({ version: "0.1.2", close, downloadAndInstall }),
  });
  expect(await updates.check(true)).toBe("0.1.2");
  expect(updates.state.versionFailed).toBe(true);
  expect(downloadAndInstall).not.toHaveBeenCalled();
  updates.destroy();
  await Promise.resolve();
  expect(close).toHaveBeenCalledTimes(1);
});

test("재확인은 이전 업데이트 리소스를 해제하고 종료 뒤 늦은 응답도 해제한다", async () => {
  const close = vi.fn().mockResolvedValue(undefined);
  const release = { version: "0.1.2", close, downloadAndInstall: vi.fn() };
  let resolve!: (value: typeof release) => void;
  const check = vi
    .fn<UpdateDeps["check"]>()
    .mockResolvedValueOnce(release)
    .mockImplementationOnce(
      () =>
        new Promise((done) => {
          resolve = done;
        }),
    );
  const updates = createUpdateStore({ getVersion: async () => "0.1.1", check });
  await updates.check();
  const pending = updates.check();
  await Promise.resolve();
  await Promise.resolve();
  expect(close).toHaveBeenCalledTimes(1);
  updates.destroy();
  resolve(release);
  await pending;
  expect(close).toHaveBeenCalledTimes(2);
  expect(updates.state.phase).toBe("checking");
});

test("설치 중 종료하면 늦은 진행은 버리고 설치가 끝난 뒤 리소스를 한 번 해제한다", async () => {
  const callbacks: ((event: DownloadEvent) => void)[] = [];
  const completions: (() => void)[] = [];
  const close = vi.fn().mockResolvedValue(undefined);
  const updates = createUpdateStore({
    getVersion: async () => "0.1.1",
    check: async () => ({
      version: "0.1.2",
      close,
      downloadAndInstall(progress) {
        if (!progress) throw new Error("진행 콜백이 필요합니다");
        callbacks.push(progress);
        return new Promise<void>((resolve) => completions.push(resolve));
      },
    }),
  });
  await updates.check();
  const pending = updates.install();
  expect(callbacks).toHaveLength(1);
  callbacks[0]({ event: "Started", data: { contentLength: 200 } });
  callbacks[0]({ event: "Progress", data: { chunkLength: 50 } });
  const before = updates.state;
  const changed = vi.fn();
  updates.subscribe(changed);
  updates.destroy();
  expect(close).not.toHaveBeenCalled();
  callbacks[0]({ event: "Progress", data: { chunkLength: 100 } });
  callbacks[0]({ event: "Finished" });
  completions[0]();
  await pending;
  expect(updates.state).toBe(before);
  expect(changed).not.toHaveBeenCalled();
  expect(close).toHaveBeenCalledTimes(1);
});
