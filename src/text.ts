import phrases from "../design/handoff/phrases.ko.json";

/** 문구 틀의 {자리} 를 채운다. 값이 없는 자리는 빈 글자 */
export function fill(tpl: string, vars: Record<string, string | number>): string {
  return tpl.replace(/\{(\w+)\}/g, (_, k: string) => String(vars[k] ?? ""));
}

/**
 * 경과 시간 글자. 1분 미만은 withSec 일 때만 "34초", 아니면 "0분".
 * 음수·NaN 은 0 으로 본다. 정각은 "13시간", 아니면 "13시간 20분".
 */
export function durationText(sec: number, withSec: boolean): string {
  const u = phrases.units;
  const s = sec >= 0 ? Math.floor(sec) : 0; // NaN 도 여기서 0
  if (withSec && s < 60) return fill(u.sec, { n: s });
  const min = Math.floor(s / 60);
  if (min >= 60) {
    const m = min % 60;
    return m ? fill(u.hourMin, { h: Math.floor(min / 60), m }) : fill(u.hour, { n: min / 60 });
  }
  const rest = s % 60;
  return withSec && rest
    ? `${fill(u.min, { n: min })} ${fill(u.sec, { n: rest })}`
    : fill(u.min, { n: min });
}

/**
 * 문장의 `{elapsed}` 를 채운다. 백엔드가 자리를 둔 문장(flag)에서만 채우고,
 * 에이전트가 쓴 글 속의 같은 글자나 백틱 코드 조각 안은 그대로 둔다 (docs/CONTRACTS.md C10).
 */
export function fillElapsed(sentence: string, sec: number | null, flag: boolean): string {
  if (!flag || !sentence.includes("{elapsed}")) return sentence;
  const text = sec === null ? "" : durationText(sec, true);
  return sentence
    .split("`")
    .map((part, i) => (i % 2 === 0 ? part.split("{elapsed}").join(text) : part))
    .join("`");
}

/** 시각 HH:MM (지역 시간) */
export function hhmm(ms: number): string {
  const d = new Date(ms);
  return `${String(d.getHours()).padStart(2, "0")}:${String(d.getMinutes()).padStart(2, "0")}`;
}
