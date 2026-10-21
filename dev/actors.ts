// 개발용 직원 액터 확인 페이지 (npm run dev 에서 /dev/actors.html). 앱 빌드에 들어가지 않는다.
import { mountOffice } from "../src/office/renderer.ts";
import type {
  EmployeeDto,
  OfficeDto,
  OfficeListDto,
  Pose,
  Rank,
  Role,
  Room,
  TeamDto,
} from "../src/types/ipc.ts";

// 이름표의 경과 시간이 보이게 몇 분 전
const nowIso = () => new Date().toISOString();
const since = new Date(Date.now() - 154_000).toISOString();

function emp(
  agentId: string,
  name: string,
  rank: Rank,
  role: Role,
  room: Room,
  pose: Pose,
  sentence: string,
  o: Partial<EmployeeDto> = {},
): EmployeeDto {
  return {
    agentId,
    name,
    rank,
    role,
    model: null,
    description: null,
    activity: {
      sentence,
      room,
      since,
      expectedSecs: null,
      pose,
      confidence: "Confirmed",
      extra: 0,
      elapsed: sentence.includes("{elapsed}"),
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
    jobs: 0,
    meeting: null,
    review: null,
    ...o,
  };
}

// 병렬 호출 수 (+n 배지)
function parallel(e: EmployeeDto, extra: number): EmployeeDto {
  return { ...e, activity: { ...e.activity, extra } };
}

function inferred(e: EmployeeDto): EmployeeDto {
  return { ...e, activity: { ...e.activity, confidence: "Inferred" } };
}

function team(
  teamId: string,
  building: string,
  deskSlot: number | null,
  employees: EmployeeDto[],
): TeamDto {
  return {
    teamId,
    title: teamId,
    building,
    hostBadge: null,
    delayed: false,
    folder: "app",
    started: since,
    lastActivity: since,
    phase: "Working",
    workType: "NewDev",
    deskSlot,
    employees,
    props: {} as TeamDto["props"],
    incident: null,
    idleAlive: false,
  };
}

const LARGE: OfficeDto = {
  stateSeq: 1,
  officeId: "dev-large",
  rebuilt: false,
  shared: { incident: false, night: false } as OfficeDto["shared"],
  cues: [],
  teams: [
    team("t1", "A", 0, [
      emp("L-boss", "김부장", "Bujang", "Backend", "TeamLead", "Type", "보고서 작성 중"),
      // 말풍선은 4초만 뜬다. 다시 보려면 페이지를 새로 고친다
      emp("L-dev0", "이대리", "Daeri", "Frontend", "Dev", "Type", "app.ts 수정 중", {
        seat: 0,
        bubble: { at: nowIso(), say: { kind: "Say", text: "린트부터 고칠게요" } },
      }),
      // +n 배지: 병렬 호출 중
      parallel(
        emp("L-par", "서대리", "Daeri", "Backend", "Dev", "Read", "열람 중 (외 2건)", { seat: 1 }),
        2,
      ),
      emp("L-dev3", "박사원", "Sawon", "Qa", "Dev", "Sit", "출근 중 · 자리 찾는 중", {
        seat: 3,
        bubble: { at: nowIso(), say: { kind: "Rank" } },
      }),
      emp(
        "L-temp",
        "최사원",
        "Sawon",
        "Trainee",
        "Dev",
        "Think",
        "턱 괴고 고민 중인 듯 ({elapsed})",
        { seat: 6 },
      ),
      emp("L-lib", "정과장", "Gwajang", "Writer", "Archive", "Sit", "'todo' 전사 수색 중", {
        bubble: { at: nowIso(), say: { kind: "NewVendor", server: "figma" } },
      }),
      emp(
        "L-build",
        "한대리",
        "Daeri",
        "DevOps",
        "Build",
        "Squat",
        "`cargo build` 실행 중 · {elapsed}째",
      ),
      emp("L-test", "오사원", "Sawon", "Qa", "Test", "Sit", "`npm test` 돌리는 중 · {elapsed}째"),
      emp("L-review", "윤과장", "Gwajang", "Backend", "Review", "Sit", "PR 리뷰 중 · 빨간펜 2개", {
        review: { pens: 2, approved: false },
      }),
      emp(
        "L-meet",
        "장대리",
        "Daeri",
        "Frontend",
        "Meeting",
        "Sit",
        "거래처 figma와 get_file 건으로 미팅 중",
      ),
      emp("L-ship", "임사원", "Sawon", "Backend", "Shipping", "Sit", "출고 포장 중"),
      emp("L-door", "강대리", "Daeri", "Writer", "Door", "Sit", "결재판 들고 문 앞 대기", {
        bubble: { at: nowIso(), say: { kind: "PlanApproval" } },
      }),
      emp("L-inc", "조사원", "Sawon", "DevOps", "Incident", "Sit", "장애 상황실 소집"),
      emp("L-pantry", "신대리", "Daeri", "Backend", "Pantry", "Sit", "탕비실에서 게임 중인 듯", {
        retiredAt: since,
      }),
      // 탕비실 셋째부터: 콘솔 → 안마의자 → 서 있기
      emp("L-pantry2", "문사원", "Sawon", "Qa", "Pantry", "Sit", "탕비실에서 쉬는 중", {
        retiredAt: since,
      }),
      emp("L-pantry3", "배사원", "Sawon", "Qa", "Pantry", "Sit", "탕비실에서 쉬는 중", {
        retiredAt: since,
      }),
      // 추정 점선
      inferred(
        emp("L-inf", "추대리", "Daeri", "Backend", "Dev", "Think", "고민 중인 듯", {
          seat: 2,
          bubble: { at: nowIso(), say: { kind: "ThinkingDuck" } },
        }),
      ),
      emp("L-out", "외근자", "Sawon", "Backend", "Outside", "Sit", "외근 중 (안 보여야 한다)"),
    ]),
  ],
};

const SMALL: OfficeDto = {
  stateSeq: 1,
  officeId: "vendor/codex",
  rebuilt: false,
  shared: { incident: true, night: false } as OfficeDto["shared"],
  cues: [],
  teams: [
    team("a", "C", 0, [
      emp(
        "S-a",
        "가부장",
        "Bujang",
        "DevOps",
        "TeamLead",
        "Duck",
        "러버덕과 상담 중인 듯 ({elapsed})",
      ),
      emp("S-a1", "가대리", "Daeri", "Frontend", "Dev", "Type", "ui.ts 수정 중", { seat: 0 }),
      emp("S-a2", "가사원", "Sawon", "Trainee", "Build", "Sit", "셸 명령 실행 중"),
    ]),
    team("b", "C", 1, [
      emp("S-b", "나부장", "Bujang", "Frontend", "Incident", "Sit", "장애 상황실 소집"),
      emp("S-b1", "나과장", "Gwajang", "Backend", "Dev", "Sit", "열람 중", { seat: 1 }),
      emp("S-b2", "나사원", "Sawon", "Qa", "Test", "Sit", "테스트 돌리는 중"),
      emp("S-b3", "나대리", "Daeri", "Qa", "Test", "Sit", "테스트 돌리는 중 (둘째 자리)"),
    ]),
    team("c", "D", 2, [
      emp("S-c", "다부장", "Bujang", "Backend", "Door", "Sit", "계획 결재 올립니다"),
      emp("S-c1", "다대리", "Daeri", "Writer", "Archive", "Sit", "서류철 꺼내는 중"),
      emp("S-c2", "다사원", "Sawon", "Backend", "Review", "Sit", "PR 리뷰 중", {
        review: { pens: 0, approved: false },
      }),
    ]),
    team("d", "C", 4, [
      emp("S-d", "라부장", "Bujang", "Qa", "Pantry", "Sleep", "졸음"),
      emp("S-d1", "라사원", "Sawon", "Frontend", "Meeting", "Sit", "외주 매뉴얼 펼쳐보는 중", {
        meeting: { kind: "Skill" },
      }),
      emp("S-d2", "라대리", "Daeri", "DevOps", "Shipping", "Sit", "출고 포장 중"),
    ]),
    // 결재 대기: 부장이 boss_pace 에 선다 (왕복 걷기는 T22)
    team("e", "C", 3, [
      emp(
        "S-e",
        "마부장",
        "Bujang",
        "Writer",
        "TeamLead",
        "Pace",
        "요구사항 대기 중인 듯 · 사장님 자리 비움 {elapsed}",
      ),
    ]),
    // 간이 책상도 찬 팀은 카드만
    team("f", "C", null, [
      emp("S-f", "바부장", "Bujang", "Backend", "TeamLead", "Type", "안 보여야 한다"),
    ]),
  ],
};

function storeOf(office: OfficeDto) {
  const list: OfficeListDto = {
    listSeq: 1,
    rebuilt: false,
    scale: office.officeId.startsWith("vendor/") ? "Small" : "Large",
    offices: [],
    selectedId: office.officeId,
    selectRequest: 0,
  };
  return { state: { list, selectedId: office.officeId, office }, subscribe: () => () => {} };
}

Promise.all([
  mountOffice(document.getElementById("large")!, storeOf(LARGE)),
  mountOffice(document.getElementById("small")!, storeOf(SMALL)),
]).catch((e) => {
  document.body.append(`실패: ${e}`);
  console.error(e);
});
