import type { CueDto, OfficeDto } from "../types/ipc.ts";

/** 과거 사건은 예약하지 않는다. 표시할 때 준비가 안 됐으면 그 소리는 버린다. */
export class OfficeSound {
  private context: AudioContext | null = null;
  private enabled = false;
  private failed = false;
  private preparing = false;
  private hidden = false;
  private selected: string | null = null;
  private office: OfficeDto | null = null;
  private generation = 0;
  private eligible = new WeakMap<CueDto, number>();
  private playing = new Map<OscillatorNode, GainNode>();
  private lastKey = -Infinity;

  constructor(private createContext: () => AudioContext = () => new AudioContext()) {}

  setEnabled(on: boolean): void {
    if (this.enabled === on) return;
    this.enabled = on;
    this.stop();
  }

  /** 클릭·키 입력의 동기 부분에서만 부른다. 저장 IPC 완료 뒤에는 사용자 제스처가 끝난다. */
  unlock(): void {
    if (!this.enabled || this.failed || this.preparing) return;
    try {
      if (!this.context) {
        this.context = this.createContext();
        this.context.addEventListener("statechange", () => {
          if (this.context?.state !== "running") this.stop();
        });
      }
      if (this.context.state === "running") return;
      this.preparing = true;
      void this.context
        .resume()
        .catch((error: unknown) => this.fail(error))
        .finally(() => {
          this.preparing = false;
        });
    } catch (error) {
      this.fail(error);
    }
  }

  private fail(error: unknown): void {
    if (!this.failed) console.error("효과음을 준비하지 못했습니다", error);
    this.failed = true;
    this.stop();
  }

  see(office: OfficeDto | null, selected: string | null, hidden: boolean): void {
    const fresh = office !== this.office;
    const boundary =
      hidden !== this.hidden ||
      selected !== this.selected ||
      office?.officeId !== this.office?.officeId ||
      (fresh && !!office?.rebuilt);
    if (boundary) this.stop();
    this.hidden = hidden;
    this.selected = selected;
    this.office = office;
    if (!fresh || boundary || !this.ready()) return;
    for (const cue of office?.cues ?? []) this.eligible.set(cue, this.generation);
  }

  cue(cue: CueDto): void {
    const eligible = this.eligible.get(cue) === this.generation;
    this.eligible.delete(cue);
    if (!eligible || !this.ready()) return;
    if (cue.kind === "BossVisit") this.tone(740, 980, 0.22, 0.025, "sine");
    else if (cue.kind === "Siren" && cue.on) this.tone(520, 880, 0.3, 0.025, "triangle");
    else if (cue.kind === "Shake") this.tone(1320, 880, 0.18, 0.025, "sine");
  }

  keyboard(typing: boolean, now: number): void {
    if (!typing || !this.ready() || now - this.lastKey < 160) return;
    this.lastKey = now;
    this.tone(180, 80, 0.025, 0.012, "triangle");
  }

  private ready(): boolean {
    return (
      this.enabled &&
      !this.failed &&
      !this.hidden &&
      this.context?.state === "running" &&
      this.office !== null &&
      this.office.officeId === this.selected
    );
  }

  private stop(): void {
    this.generation++;
    for (const [source, gain] of this.playing) {
      source.stop();
      source.disconnect();
      gain.disconnect();
    }
    this.playing.clear();
  }

  private tone(
    from: number,
    to: number,
    seconds: number,
    volume: number,
    type: OscillatorType,
  ): void {
    const context = this.context!;
    const source = context.createOscillator();
    const gain = context.createGain();
    const at = context.currentTime;
    source.type = type;
    source.frequency.setValueAtTime(from, at);
    source.frequency.exponentialRampToValueAtTime(to, at + seconds);
    gain.gain.setValueAtTime(0, at);
    gain.gain.linearRampToValueAtTime(volume, at + 0.004);
    gain.gain.exponentialRampToValueAtTime(0.0001, at + seconds);
    source.connect(gain);
    gain.connect(context.destination);
    this.playing.set(source, gain);
    source.onended = () => {
      this.playing.delete(source);
      source.disconnect();
      gain.disconnect();
    };
    source.start(at);
    source.stop(at + seconds);
  }
}
