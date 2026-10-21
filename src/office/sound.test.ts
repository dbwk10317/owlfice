import { expect, test, vi } from "vite-plus/test";
import { office } from "../test-office.ts";
import type { CueDto, OfficeDto } from "../types/ipc.ts";
import { cueCollector } from "./renderer.ts";
import { OfficeSound } from "./sound.ts";

function audio() {
  const notes: { frequency: number[]; gain: number[]; stopped: boolean; length: number }[] = [];
  const listeners: (() => void)[] = [];
  const context = {
    state: "running",
    currentTime: 1,
    destination: {},
    resume: vi.fn(async () => {
      context.state = "running";
    }),
    addEventListener: (_: string, fn: () => void) => listeners.push(fn),
    createOscillator() {
      const note = { frequency: [] as number[], gain: [] as number[], stopped: false, length: 0 };
      notes.push(note);
      return {
        type: "sine",
        frequency: {
          setValueAtTime: (n: number) => note.frequency.push(n),
          exponentialRampToValueAtTime: (n: number) => note.frequency.push(n),
        },
        connect() {},
        disconnect() {},
        start() {},
        stop(at?: number) {
          if (at === undefined) note.stopped = true;
          else note.length = at - context.currentTime;
        },
        onended: null,
      };
    },
    createGain() {
      const values = notes[notes.length - 1].gain;
      return {
        gain: {
          setValueAtTime: (n: number) => values.push(n),
          linearRampToValueAtTime: (n: number) => values.push(n),
          exponentialRampToValueAtTime: (n: number) => values.push(n),
        },
        connect() {},
        disconnect() {},
      };
    },
  };
  const create = vi.fn(() => context as unknown as AudioContext);
  const sound = new OfficeSound(create);
  let seq = 0;
  const dto = (cues: CueDto[] = []) => office("one", ++seq, [], cues);
  sound.see(dto(), "one", false);
  function emit(c: CueDto): void {
    sound.see(dto([c]), "one", false);
    sound.cue(c);
  }
  function enable(): void {
    sound.setEnabled(true);
    sound.unlock();
  }
  return {
    context,
    notes,
    sound,
    create,
    dto,
    emit,
    enable,
    stateChange: () => listeners.forEach((f) => f()),
  };
}
const shake = (): CueDto => ({ kind: "Shake", teamId: "t", agent: "a" });
const siren = (on: boolean): CueDto => ({ kind: "Siren", teamId: "t", on });
const visit = (): CueDto => ({ kind: "BossVisit", teamId: "t", text: "합성 방문" });

test("기본 꺼짐은 context도 만들지 않으며 켠 뒤 네 소리는 짧고 작은 음량이다", () => {
  const a = audio();
  a.sound.unlock();
  a.emit(shake());
  a.sound.keyboard(true, 0);
  expect(a.create).not.toHaveBeenCalled();
  expect(a.notes).toHaveLength(0);
  a.enable();
  a.emit(visit());
  a.emit(siren(true));
  a.emit(shake());
  a.sound.keyboard(true, 0);
  a.emit(siren(false));
  expect(a.notes).toHaveLength(4);
  expect(new Set(a.notes.map((n) => n.frequency.join())).size).toBe(4);
  expect(a.notes.every((n) => n.length <= 0.301 && Math.max(...n.gain) <= 0.025)).toBe(true);
  a.sound.setEnabled(false);
  expect(a.notes.every((n) => n.stopped)).toBe(true);
  a.emit(visit());
  a.sound.keyboard(true, 1000);
  expect(a.notes).toHaveLength(4);
});

test("동일 DTO와 이미 소비한 cue를 다시 넘겨도 중복 소리가 나지 않는다", () => {
  const a = audio();
  a.enable();
  const c = shake();
  const o = a.dto([c]);
  a.sound.see(o, "one", false);
  a.sound.cue(c);
  a.sound.see(o, "one", false);
  a.sound.cue(c);
  expect(a.notes).toHaveLength(1);
});

test("음소거 전후 렌더 틱 사이에 모인 큐도 과거 소리를 되살리지 않는다", () => {
  const a = audio();
  a.enable();
  const collected = cueCollector();
  const before = a.dto([shake()]);
  a.sound.see(before, "one", false);
  collected.see(before, false);
  a.sound.setEnabled(false);
  const muted = a.dto([siren(true)]);
  a.sound.see(muted, "one", false);
  collected.see(muted, false);
  a.enable();
  const after = a.dto([visit()]);
  a.sound.see(after, "one", false);
  collected.see(after, false);
  for (const c of [...collected.take(after), ...after.cues]) a.sound.cue(c);
  expect(a.notes).toHaveLength(1);
  expect(a.notes[0].frequency[0]).toBe(740);
});

test.each(["hidden", "switch", "rebuilt"])(
  "%s 경계는 재생과 남은 사건음을 끊고 현재 타건은 다시 허용한다",
  (boundary) => {
    const a = audio();
    a.enable();
    const c = shake();
    const o = a.dto([c]);
    a.sound.see(o, "one", false);
    a.sound.keyboard(true, 0);
    const next: OfficeDto = boundary === "rebuilt" ? { ...a.dto([siren(true)]), rebuilt: true } : o;
    a.sound.see(next, boundary === "switch" ? "two" : "one", boundary === "hidden");
    expect(a.notes[0].stopped).toBe(true);
    a.sound.cue(c);
    for (const cue of next.cues) a.sound.cue(cue);
    expect(a.notes).toHaveLength(1);
    a.sound.see(next, "one", false);
    a.sound.keyboard(true, 1000);
    expect(a.notes).toHaveLength(2);
  },
);

test("준비 중 사건은 버리고 resume 뒤에도 재생하지 않는다", async () => {
  const a = audio();
  let resolve!: () => void;
  a.context.state = "suspended";
  a.context.resume.mockImplementation(
    () =>
      new Promise<void>((r) => {
        resolve = r;
      }),
  );
  a.enable();
  const c = shake();
  a.sound.see(a.dto([c]), "one", false);
  a.sound.cue(c);
  a.sound.unlock();
  expect(a.context.resume).toHaveBeenCalledTimes(1);
  a.context.state = "running";
  resolve();
  await Promise.resolve();
  a.sound.cue(c);
  expect(a.notes).toHaveLength(0);
  a.emit(shake());
  expect(a.notes).toHaveLength(1);
});

test("resume 실패는 한 번 기록하고 다음 제스처에서도 무한 재시도하지 않는다", async () => {
  const a = audio();
  const error = vi.spyOn(console, "error").mockImplementation(() => {});
  try {
    a.context.state = "suspended";
    a.context.resume.mockRejectedValue(new Error("합성 실패"));
    a.enable();
    await Promise.resolve();
    await Promise.resolve();
    a.sound.unlock();
    a.emit(shake());
    expect(a.context.resume).toHaveBeenCalledTimes(1);
    expect(error).toHaveBeenCalledTimes(1);
    expect(a.notes).toHaveLength(0);
  } finally {
    error.mockRestore();
  }
});

test("context가 중단되면 진행 중 소리도 폐기하여 resume 때 이어 울리지 않는다", () => {
  const a = audio();
  a.enable();
  a.emit(visit());
  a.context.state = "suspended";
  a.stateChange();
  expect(a.notes[0].stopped).toBe(true);
});

test("현재 Type이 있을 때만 전체 160ms당 한 번, 멈췄다 돌아와도 밀린 타건은 없다", () => {
  const a = audio();
  a.enable();
  a.sound.keyboard(false, 0);
  expect(a.notes).toHaveLength(0);
  for (const at of [0, 0, 50, 159, 160, 160, 200]) a.sound.keyboard(true, at);
  expect(a.notes).toHaveLength(2);
  a.sound.keyboard(false, 10_000);
  a.sound.keyboard(true, 20_000);
  expect(a.notes).toHaveLength(3);
});
