import { mountOffice } from "../src/office/renderer.ts";
import { act, emp, office, team } from "../src/test-office.ts";
import type { OfficeDto, OfficeListDto } from "../src/types/ipc.ts";

const now = () => new Date().toISOString();
const teams = ["가", "나"].map((id, i) =>
  team(
    id,
    [
      emp(`${id}-부장`, {
        name: `${id}부장`,
        rank: "Bujang",
        activity: act({ room: "TeamLead", pose: "Type" }),
      }),
      emp(`${id}-대리`, { name: `${id}대리`, seat: i, activity: act({ room: "Dev" }) }),
    ],
    {
      deskSlot: i,
      title: "야근하지않고정확하게일하는우리회사",
      started: new Date(Date.now() - (i + 1) * 3_600_000).toISOString(),
    },
  ),
);
const initial = office("vendor/walls", 1, teams);
initial.shared.frameTitle = teams[0].title;
initial.shared.stats = { commits: 1, filesEdited: 2, commands: 3, turns: 4 };
initial.roomRecent = [
  {
    room: "Dev",
    events: Array.from({ length: 10 }, (_, i) => ({
      teamId: teams[i % 2].teamId,
      agentId: `${teams[i % 2].teamId}-대리`,
      activity: act({
        since: new Date(Date.now() - i * 60_000).toISOString(),
        sentence: `<script>실행되지 않는 원문</script> · 파일 ${i} 수정 완료`,
        confidence: i % 2 ? "Inferred" : "Confirmed",
      }),
    })),
  },
];
const list: OfficeListDto = {
  listSeq: 1,
  rebuilt: false,
  scale: "Small",
  offices: [],
  selectedId: initial.officeId,
  selectRequest: 0,
};
const listeners = new Set<() => void>();
const store = {
  state: { list, selectedId: initial.officeId, office: initial as OfficeDto | null },
  subscribe(fn: () => void) {
    listeners.add(fn);
    return () => listeners.delete(fn);
  },
};
const status = document.getElementById("status")!;
function set(next: OfficeDto | null): void {
  store.state.office = next;
  store.state.selectedId = next?.officeId ?? "";
  listeners.forEach((fn) => fn());
}
function edit(fn: (o: OfficeDto) => void): void {
  const o = structuredClone(store.state.office ?? initial);
  o.stateSeq++;
  o.cues = [];
  o.rebuilt = false;
  fn(o);
  set(o);
}
function button(label: string, click: () => void): void {
  const el = document.createElement("button");
  el.textContent = label;
  el.addEventListener("click", click);
  document.getElementById("controls")!.append(el);
}
button("기본 원본", () => set(null));
button("두 팀 / 1·2·3·4", () => set(structuredClone(initial)));
button("0·9·10·12345", () =>
  edit((o) => {
    o.shared.stats = { commits: 0, filesEdited: 9, commands: 10, turns: 12345 };
  }),
);
let swaps = 0;
button("사다리 제목 교체", () =>
  edit((o) => {
    const title = ++swaps % 2 ? "정확하게일하고정시에퇴근합시다" : "함께만드는새로운사무실";
    o.shared.frameTitle = title;
    o.teams[0].title = title;
    o.teams[0].lastActivity = now();
    o.cues = [{ kind: "FrameSwap", teamId: o.teams[0].teamId, title }];
    status.textContent = `교체 요청: ${title}`;
  }),
);
button("재구성 즉시 교체", () =>
  edit((o) => {
    o.shared.frameTitle = "즉시교체";
    o.rebuilt = true;
  }),
);
button("사장님 긴 메시지", () =>
  edit((o) => {
    const text =
      "<img src=x onerror=alert(1)> 원문으로만 표시합니다.\n" +
      "마지막 메시지 전문입니다. 줄을 내려도 끝까지 볼 수 있어야 합니다.\n".repeat(35);
    o.teams[1].props.bossVisit = text;
    o.cues = [{ kind: "BossVisit", teamId: o.teams[1].teamId, text }];
  }),
);
button("사무실 전환", () =>
  edit((o) => {
    o.officeId = o.officeId === "vendor/walls" ? "vendor/other" : "vendor/walls";
    o.shared.frameTitle = "새방제목";
  }),
);
await mountOffice(document.getElementById("office")!, store, {
  onSession: (_, id) => {
    status.textContent = store.state.office?.teams.find((t) => t.teamId === id)?.title ?? id;
  },
  onEmployee: (_, teamId, agentId) => {
    status.textContent = `직원 클릭: ${teamId} / ${agentId}`;
  },
});
