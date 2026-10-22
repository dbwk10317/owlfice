import "../design/handoff/tokens.css";
import "../src/styles/app.css";
import { mountOffice } from "../src/office/renderer.ts";
import { OfficeSound } from "../src/office/sound.ts";
import { act, emp, office, team } from "../src/test-office.ts";
import type { CueDto, OfficeDto, OfficeListDto } from "../src/types/ipc.ts";

const sound = new OfficeSound();
let enabled = false;
let typing = false;
let selected = "one";
let seq = 0;
const listeners = new Set<() => void>();
const button = (id: string) => document.getElementById(id) as HTMLButtonElement;
const status = document.getElementById("status")!;
function snapshot(cues: CueDto[] = [], rebuilt = false): OfficeDto {
  const o = office(
    selected,
    ++seq,
    [
      team(
        "sample",
        [
          emp("boss", { rank: "Bujang", activity: act({ room: "TeamLead", pose: "Sit" }) }),
          emp("worker", { seat: 0, activity: act({ pose: typing ? "Type" : "Sit" }) }),
          emp("worker-two", { seat: 1, activity: act({ pose: typing ? "Type" : "Sit" }) }),
        ],
        { deskSlot: 0 },
      ),
    ],
    cues,
  );
  o.rebuilt = rebuilt;
  return o;
}
const state: { list: OfficeListDto; office: OfficeDto; selectedId: string } = {
  list: {
    listSeq: 1,
    scale: "Large",
    selectedId: selected,
    selectRequest: 0,
    rebuilt: false,
    offices: [],
  },
  office: snapshot(),
  selectedId: selected,
};
const store = {
  state,
  subscribe(fn: () => void) {
    listeners.add(fn);
    return () => {
      listeners.delete(fn);
    };
  },
};
function publish(cues: CueDto[] = [], rebuilt = false): void {
  state.office = snapshot(cues, rebuilt);
  state.selectedId = selected;
  for (const fn of listeners) fn();
  status.textContent = `${selected} · ${enabled ? "소리 켜짐" : "무음"} · ${typing ? "직원 두 명 타자 (합쳐서 160ms 간격)" : "타자 멈춤"} · ${rebuilt ? "조용한 재구성" : cues.map((c) => c.kind).join(", ")}`;
}
function setSound(on: boolean): void {
  enabled = on;
  sound.setEnabled(on);
  if (on) sound.unlock();
  button("sound").textContent = on ? "소리 끄기" : "소리 켜기";
  button("sound").setAttribute("aria-pressed", String(on));
}
button("sound").onclick = () => {
  setSound(!enabled);
  publish();
};
button("typing").onclick = () => {
  typing = !typing;
  button("typing").textContent = typing ? "타자 멈추기" : "타자 시작";
  button("typing").setAttribute("aria-pressed", String(typing));
  publish();
};
const ding = (): CueDto => ({ kind: "Shake", teamId: "sample", agent: "worker" });
button("visit").onclick = () =>
  publish([{ kind: "BossVisit", teamId: "sample", text: "합성 방문" }]);
button("siren").onclick = () => publish([{ kind: "Siren", teamId: "sample", on: true }]);
button("ding").onclick = () => publish([ding()]);
button("off").onclick = () => publish([{ kind: "Siren", teamId: "sample", on: false }]);
button("rebuild").onclick = () => publish([ding()], true);
button("switch").onclick = () => {
  selected = selected === "one" ? "two" : "one";
  publish([ding()]);
};
button("past").onclick = () => {
  setSound(false);
  publish([ding()]);
  setSound(true);
  status.textContent = "다음 렌더 틱에 이전 땡이 울리지 않아야 합니다.";
};
await mountOffice(document.getElementById("stage")!, store, { sound });
document.body.dataset.ready = "true";
status.textContent = "준비 완료 · 명시 버튼으로만 소리를 켭니다.";
