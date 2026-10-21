// 개발용 서브 종료·재출근·분갈이·액자 교체 연출 확인 페이지 (npm run dev 에서 /dev/life.html). 앱 빌드에 들어가지 않는다.
// 버튼이 직원 방·퇴장 시각·화분 값·제목을 바꿔 S20 S38 S39 S60 을 보여 준다. 걷는 장면은 앞 상태를 먼저 보낸 뒤 다음 상태를 보낸다.
import { mountOffice } from "../src/office/renderer.ts";
import type {
  CueDto,
  EmployeeDto,
  OfficeDto,
  OfficeListDto,
  Pose,
  Room,
} from "../src/types/ipc.ts";

const T = "t1";
const iso = (msAgo = 0) => new Date(Date.now() - msAgo).toISOString();

function emp(
  agentId: string,
  rank: "Bujang" | "Daeri",
  room: Room,
  pose: Pose,
  o: Partial<EmployeeDto> = {},
): EmployeeDto {
  const since = iso();
  return {
    agentId,
    name: rank === "Bujang" ? "김부장" : agentId,
    rank,
    role: "Backend",
    model: null,
    description: null,
    activity: {
      sentence: `${room} ${pose}`,
      room,
      since,
      expectedSecs: null,
      pose,
      confidence: "Confirmed",
      extra: 0,
      elapsed: false,
    },
    recent: [],
    counters: { read: 0, edit: 0, shell: 0, outside: 0, meeting: 0, errors: 0 },
    lastSay: null,
    hiredAt: since,
    retiredAt: null,
    seat: null,
    parent: null,
    depth: 1,
    confidence: "Confirmed",
    contextUsed: null,
    contextWindow: null,
    bubble: null,
    ...o,
  };
}

const lead = () => emp("boss", "Bujang", "TeamLead", "Sit");
/** 자리에 앉은 서브 / 퇴장해 탕비실에 있는 서브 */
const desk = (id: string, seat: number) => emp(id, "Daeri", "Dev", "Type", { seat });
const gone = (id: string) => emp(id, "Daeri", "Pantry", "Sit", { retiredAt: iso() });

let seq = 1;
function office(subs: EmployeeDto[], plant: number, title: string, cues: CueDto[] = []): OfficeDto {
  const since = iso();
  return {
    stateSeq: seq++,
    officeId: "dev-life",
    rebuilt: false,
    shared: {
      incident: false,
      night: false,
      light: "Off",
      safeSince: since,
      trash: 0,
      pizza: 0,
      frameTitle: title,
      kanban: [],
    } as unknown as OfficeDto["shared"],
    cues,
    teams: [
      {
        teamId: T,
        title,
        building: "A",
        hostBadge: null,
        delayed: false,
        folder: "app",
        started: since,
        lastActivity: since,
        phase: "Working",
        workType: "NewDev",
        deskSlot: 0,
        employees: [lead(), ...subs],
        props: { plant } as OfficeDto["teams"][number]["props"],
        incident: null,
      },
    ],
  };
}

const list: OfficeListDto = {
  listSeq: 1,
  rebuilt: false,
  scale: "Large",
  offices: [],
  selectedId: "dev-life",
  selectRequest: 0,
};
const listeners = new Set<() => void>();
const store = {
  state: {
    list,
    selectedId: "dev-life",
    office: office([desk("a", 0), desk("b", 1)], 0.9, "제목"),
  },
  subscribe(fn: () => void) {
    listeners.add(fn);
    return () => listeners.delete(fn);
  },
};
const show = (o: OfficeDto) => {
  store.state.office = o;
  for (const fn of listeners) fn();
};
const cue = (c: Record<string, unknown>) => ({ teamId: T, ...c }) as CueDto;
const then = (first: () => void, next: () => void) => {
  first();
  setTimeout(next, 150);
};

const sitting = () => show(office([desk("a", 0), desk("b", 1)], 0.9, "제목"));
const STEPS: [string, () => void][] = [
  ["처음: 둘 다 자리에", sitting],
  [
    "서브 a 종료 (S20: 콘솔 sit, 게임기, 퇴근 자석)",
    () =>
      then(sitting, () =>
        show(office([gone("a"), desk("b", 1)], 0.9, "제목", [cue({ kind: "Retire", agent: "a" })])),
      ),
  ],
  [
    "서브 b도 종료 (안마의자 sleep)",
    () => show(office([gone("a"), gone("b")], 0.9, "제목", [cue({ kind: "Retire", agent: "b" })])),
  ],
  [
    "서브 a 재출근 (S60: 자리로 걸어옴, 출근 자석)",
    () =>
      show(office([desk("a", 0), gone("b")], 0.9, "제목", [cue({ kind: "Rehire", agent: "a" })])),
  ],
  [
    "분갈이 (S38: 화분이 잠깐 비었다 s0)",
    () =>
      then(sitting, () =>
        show(office([desk("a", 0), desk("b", 1)], 0.05, "제목", [cue({ kind: "Repot" })])),
      ),
  ],
  [
    "제목 변경 (S39: 사다리 4초)",
    () =>
      then(sitting, () =>
        show(
          office([desk("a", 0), desk("b", 1)], 0.9, "새 제목", [
            cue({ kind: "FrameSwap", title: "새 제목" }),
          ]),
        ),
      ),
  ],
];
const buttons = document.getElementById("buttons")!;
for (const [label, run] of STEPS) {
  const b = document.createElement("button");
  b.textContent = label;
  b.onclick = run;
  buttons.append(b);
}

mountOffice(document.getElementById("large")!, store).catch((e) => {
  document.body.append(`실패: ${e}`);
  console.error(e);
});
