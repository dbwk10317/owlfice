import type { OfficeDto, Room } from "../types/ipc.ts";
import { durationText, fillElapsed, hhmm } from "../text.ts";
import { attendanceColumn } from "./props.ts";

const ROOMS: Record<string, Room[]> = {
  library: ["Archive"],
  build: ["Build"],
  test: ["Test"],
  boss: ["TeamLead"],
  dev: ["Dev"],
  review: ["Review"],
  incident: ["Incident"],
  meeting: ["Meeting"],
  conference: ["Conference"],
  entrance: ["Door", "Shipping", "Outside"],
  pantry: ["Pantry"],
};

export function propTooltip(id: string, office: OfficeDto | null, now: number): string {
  if (!office) return "";
  if (id === "clock") {
    const oldest = Math.min(
      ...office.teams.map((t) => Date.parse(t.started)).filter(Number.isFinite),
    );
    if (!Number.isFinite(oldest)) return "";
    return `벽시계\n시작 ${new Date(oldest).toLocaleString("ko-KR")}\n경과 ${durationText((now - oldest) / 1000, true)}`;
  }
  if (id === "attendance") {
    const cols = ["출근", "외근", "외출", "퇴근"];
    return [
      "근태판",
      ...office.teams.flatMap((t) =>
        t.employees.map((e) => `${e.name} · ${cols[attendanceColumn(t, e)]}`),
      ),
    ].join("\n");
  }
  if (id === "stats_board") {
    const s = office.shared.stats;
    return `오늘의 통계 · 세션 누계\n커밋 ${s.commits}\n수정 파일 ${s.filesEdited}\n실행 명령 ${s.commands}\n에너지 드링크(턴) ${s.turns}\n판: 왼쪽 위 커밋 · 오른쪽 위 파일\n왼쪽 아래 명령 · 오른쪽 아래 턴\n+ 표시는 10 이상`;
  }
  return "";
}

export function roomTooltip(
  key: string,
  name: string,
  office: OfficeDto | null,
  now: number,
): string {
  const rooms = ROOMS[key] ?? [];
  const events = (office?.roomRecent ?? [])
    .filter((r) => rooms.includes(r.room))
    .flatMap((r) => r.events)
    .sort((a, b) => Date.parse(b.activity.since) - Date.parse(a.activity.since))
    .slice(0, 10);
  return [
    name,
    ...events.map(({ teamId, agentId, activity: a }) => {
      const emp = office?.teams
        .find((t) => t.teamId === teamId)
        ?.employees.find((e) => e.agentId === agentId);
      const at = Date.parse(a.since);
      return `${hhmm(at)} ${emp?.name ?? agentId} · ${fillElapsed(a.sentence, (now - at) / 1000, a.elapsed)}${a.confidence === "Inferred" ? " (추정)" : ""}`;
    }),
    ...(events.length ? [] : ["최근 사건이 없습니다."]),
  ].join("\n");
}

/** 원문은 textContent로만 넣고, 캔버스 배율과 무관한 화면 좌표로 제한한다. */
export class CanvasTooltip {
  readonly el: HTMLDivElement;
  private source: HTMLElement | null = null;

  constructor(host: HTMLElement) {
    this.el = document.createElement("div");
    this.el.className = "ow-nametag ow-canvas-tooltip";
    this.el.id = `ow-canvas-tooltip-${++tooltipId}`;
    this.el.role = "tooltip";
    this.el.tabIndex = 0;
    this.el.hidden = true;
    host.append(this.el);
    document.addEventListener("keydown", (e) => {
      if (e.key === "Escape") this.close();
    });
    document.addEventListener("pointerdown", (e) => {
      if (!this.el.contains(e.target as Node) && !this.source?.contains(e.target as Node))
        this.close();
    });
  }

  show(text: string, x: number, y: number, source?: HTMLElement): void {
    this.close();
    if (!text) return;
    this.source = source ?? null;
    source?.setAttribute("aria-describedby", this.el.id);
    this.el.textContent = text;
    this.el.hidden = false;
    this.el.scrollTop = 0;
    const width = document.documentElement.clientWidth || window.innerWidth;
    const height = document.documentElement.clientHeight || window.innerHeight;
    this.el.style.maxWidth = `${Math.max(0, Math.min(380, width - 16))}px`;
    this.el.style.maxHeight = `${Math.max(0, height - 16)}px`;
    const rect = this.el.getBoundingClientRect();
    this.el.style.left = `${Math.max(8, Math.min(x + 8, width - rect.width - 8))}px`;
    this.el.style.top = `${Math.max(8, Math.min(y + 8, height - rect.height - 8))}px`;
  }

  close(): void {
    this.source?.removeAttribute("aria-describedby");
    this.source = null;
    this.el.hidden = true;
  }
}

let tooltipId = 0;
