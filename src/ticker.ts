// 사내 방송 티커 (PLAN 3.5, HANDOFF 4.5). 직원 행적(recent)에서 새로 생긴 줄을 사건으로 모아 시간순으로 흘린다.
import phrases from "../design/handoff/phrases.ko.json";
import { reducedMotion } from "./loading.ts";
import { fill, hhmm } from "./text.ts";
import type { OfficeDto, Rank, TeamDto } from "./types/ipc.ts";

/** 오래된 사건은 버린다. 흐르는 줄이 끝없이 길어지지 않게 */
export const TICKER_MAX = 20;

export interface TickerEvent {
  key: string;
  agentId: string;
  at: number;
  time: string;
  text: string;
}

const RANK_LABEL: Record<Rank, string> = {
  Bujang: phrases.ranks.bujang,
  Gwajang: phrases.ranks.gwajang,
  Daeri: phrases.ranks.daeri,
  Sawon: phrases.ranks.sawon,
};
export const rankLabel = (r: Rank): string => RANK_LABEL[r];

/** "{boss} 부장 팀". 부장은 team.employees[0] */
export function teamName(team: TeamDto): string {
  const boss = team.employees[0];
  return boss ? fill(phrases.sidebar.teamName, { boss: boss.name }) : team.title;
}

/**
 * 행적 변화 수집기. 전체 스냅샷이 들어올 때마다 직원별로 "이전에 못 본 줄"만 사건으로 낸다.
 * 사무실이 바뀌면 비운다: 다른 사무실의 사건이 섞이지 않게.
 */
export function createEventCollector() {
  let officeId: string | null = null;
  // 직원마다 지난번 recent 의 키 집합만 든다. 집합은 recent 길이로 묶여 커지지 않는다
  let seen = new Map<string, Set<string>>();
  let events: TickerEvent[] = [];

  function reset(): void {
    officeId = null;
    seen = new Map();
    events = [];
  }

  function ingest(office: OfficeDto, withTeam: boolean): boolean {
    if (office.officeId !== officeId) {
      reset();
      officeId = office.officeId;
    }
    const added: TickerEvent[] = [];
    const alive = new Set<string>();
    for (const team of office.teams) {
      const tn = teamName(team);
      for (const e of team.employees) {
        alive.add(e.agentId);
        const prev = seen.get(e.agentId);
        const keys = new Set<string>();
        for (const a of e.recent) {
          const key = `${e.agentId}|${a.since}|${a.sentence}`;
          keys.add(key);
          const at = Date.parse(a.since);
          if (prev?.has(key) || Number.isNaN(at)) continue;
          const time = hhmm(at);
          // 시각은 항목 안에서 따로 그리므로 틀의 {time} 자리는 비운다
          const text = fill(withTeam ? phrases.ticker.itemTeamPerson : phrases.ticker.item, {
            time: "",
            team: tn,
            name: e.name,
            rank: RANK_LABEL[e.rank],
            event: a.sentence.split("`").join(""),
          }).trim();
          added.push({ key, agentId: e.agentId, at, time, text });
        }
        seen.set(e.agentId, keys);
      }
    }
    for (const id of seen.keys()) if (!alive.has(id)) seen.delete(id);
    if (added.length === 0) return false;
    // 같은 시각은 들어온 순서를 지킨다 (sort 는 안정 정렬)
    events = [...events, ...added].sort((a, b) => a.at - b.at).slice(-TICKER_MAX);
    return true;
  }

  return { ingest, reset, events: (): readonly TickerEvent[] => events };
}

const SPEED_FALLBACK = 40;

/** 티커 DOM. 사건이 바뀔 때만 항목을 다시 만든다 */
export function createTicker(doc: Document, onPick: (agentId: string) => void) {
  const root = doc.createElement("div");
  root.className = "ow-ticker";
  const label = doc.createElement("span");
  label.className = "ow-chip ow-ticker__label";
  label.textContent = phrases.ticker.label;
  const view = doc.createElement("div");
  view.className = "ow-ticker__view";
  const track = doc.createElement("div");
  track.className = "ow-ticker__track";
  // 읽는 쪽에는 한 번만 들려준다: 흐르는 복사본은 숨긴다
  track.setAttribute("aria-live", "off");
  view.append(track);
  root.append(label, view);

  let shown: string | null = null;

  function item(ev: TickerEvent, dup: boolean): HTMLElement {
    const b = doc.createElement("button");
    b.type = "button";
    b.className = "ow-ticker__item";
    b.tabIndex = -1;
    if (dup) b.setAttribute("aria-hidden", "true");
    const t = doc.createElement("span");
    t.className = "ow-mono ow-ticker__time";
    t.textContent = ev.time;
    b.append(t, ` ${ev.text}`);
    b.addEventListener("click", () => onPick(ev.agentId));
    return b;
  }

  function setEvents(events: readonly TickerEvent[]): void {
    const sig = events.map((e) => e.key).join("\n");
    if (sig === shown) return;
    shown = sig;
    track.textContent = "";
    if (events.length === 0) {
      track.classList.remove("is-flowing");
      track.textContent = phrases.ticker.quiet;
      return;
    }
    // 움직임 줄이기: 최신 항목 하나를 정지 표시 (HANDOFF 7절)
    if (reducedMotion()) {
      track.classList.remove("is-flowing");
      track.append(item(events[events.length - 1], false));
      return;
    }
    // 목록을 두 번 이어 붙여 끊김 없이 (HANDOFF 4.5)
    for (const dup of [false, true]) for (const ev of events) track.append(item(ev, dup));
    track.classList.add("is-flowing");
    measure();
  }

  // 숨은 동안(드로어 닫힘, 압축형)에는 폭이 0이라 보일 때 다시 재야 한다
  function measure(): void {
    if (!track.classList.contains("is-flowing")) return;
    const speed =
      Number(
        doc.defaultView
          ?.getComputedStyle(doc.documentElement)
          .getPropertyValue("--ow-ticker-speed"),
      ) || SPEED_FALLBACK;
    const half = track.scrollWidth / 2;
    if (half > 0) track.style.animationDuration = `${half / speed}s`;
  }

  setEvents([]);
  return { el: root, setEvents, remeasure: measure };
}
