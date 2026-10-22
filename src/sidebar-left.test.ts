// @vitest-environment jsdom
import { afterEach, beforeEach, expect, test, vi } from "vite-plus/test";
import { createStore } from "./store.ts";
import { emptyState, mountSidebar, popupNames, type SidebarOptions } from "./sidebar-left.ts";
import type {
  Border,
  Dot,
  OfficeDto,
  OfficeListDto,
  OfficeSummaryDto,
  Scale,
  TeamPhase,
  TeamSummaryDto,
} from "./types/ipc.ts";

// ---------- 도우미 ----------

const byClass = (root: Element, cls: string) => [...root.querySelectorAll<HTMLElement>("." + cls)];
const rows = (root: Element) => byClass(root, "ow-row");
const rowByName = (root: Element, name: string) =>
  rows(root).find((r) => byClass(r, "ow-row__name")[0]?.textContent === name)!;
const dotOf = (row: Element) => byClass(row, "ow-dot")[0];
const count = (root: Element) => root.querySelectorAll("*").length;
const fire = (el: Element, type: string) => el.dispatchEvent(new Event(type));

// ---------- 자료 ----------

const team = (id: string, over: Partial<TeamSummaryDto> = {}): TeamSummaryDto => ({
  teamId: id,
  bossName: "김하늘",
  title: "t",
  folder: "lumaweave",
  others: 3,
  started: new Date(Date.now() - 12 * 60_000).toISOString(),
  phase: "Working",
  dot: "Working",
  asking: false,
  askingCount: 0,
  border: "Plain",
  delayed: false,
  hostBadge: null,
  workType: "BugFix",
  ...over,
});
const office = (id: string, over: Partial<OfficeSummaryDto> = {}): OfficeSummaryDto => ({
  officeId: id,
  building: "B",
  floor: 2,
  room: 203,
  label: `B동 ${id}`,
  phase: "Working" as TeamPhase,
  dot: "Working" as Dot,
  asking: false,
  border: "Plain" as Border,
  delayed: false,
  teams: [team(`t-${id}`)],
  ...over,
});
let seq = 0;
const list = (
  offices: OfficeSummaryDto[],
  scale: Scale = "Large",
  over: Partial<OfficeListDto> = {},
): OfficeListDto => ({
  listSeq: ++seq,
  rebuilt: false,
  scale,
  offices,
  selectedId: null,
  selectRequest: 0,
  ...over,
});

function setup(
  first: OfficeListDto,
  width = 1200,
  onDetail?: SidebarOptions["onDetail"],
  onTeam?: SidebarOptions["onTeam"],
) {
  const store = createStore<OfficeListDto, OfficeDto>();
  store.applyWorld({ ...first, selected: null });
  const root = document.createElement("div");
  document.body.append(root);
  const widthRef = { w: width };
  const picked: [string, number][] = [];
  const dispose = mountSidebar(root, store, (id, req) => picked.push([id, req]), {
    getWidth: () => widthRef.w,
    onDetail,
    onTeam,
  });
  const listEl = byClass(root, "ow-sidebar__list")[0];
  const slot = byClass(root, "ow-toast-slot")[0];
  return { store, root, listEl, slot, widthRef, picked, dispose };
}

let roInstances: { cb: () => void }[] = [];
beforeEach(() => {
  vi.useFakeTimers();
  roInstances = [];
  vi.stubGlobal(
    "ResizeObserver",
    class {
      constructor(cb: () => void) {
        roInstances.push({ cb });
      }
      observe() {}
      disconnect() {}
    },
  );
});
afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

// ---------- 테스트 ----------

test("중소기업 팀은 다른 층과 현재 층 모두 팀 포커스를 요청한다", () => {
  const focus = vi.fn();
  const s = setup(
    list([office("vendor/Claude", { building: null, teams: [team("one")] })], "Small"),
    1200,
    undefined,
    focus,
  );
  const row = s.root.querySelector<HTMLButtonElement>(".is-team .ow-row__main")!;
  row.click();
  expect(s.picked).toHaveLength(1);
  expect(focus).toHaveBeenLastCalledWith("vendor/Claude", "one");
  row.click();
  expect(s.picked).toHaveLength(1);
  expect(focus).toHaveBeenCalledTimes(2);
  s.dispose();
});

test("추가·갱신·제거: 같은 office id 행은 같은 DOM 노드다", () => {
  const s = setup(list([office("101"), office("102")]));
  const a = rowByName(s.root, "B동 101");
  const b = rowByName(s.root, "B동 102");
  expect(dotOf(a).dataset.state).toBe("working");

  s.store.applyList(list([office("101", { dot: "Incident", border: "Failed" }), office("103")]));
  expect(rowByName(s.root, "B동 101")).toBe(a); // 갱신
  expect(dotOf(a).dataset.state).toBe("incident");
  expect(dotOf(a).dataset.ring).toBe("fail");
  expect(b.isConnected).toBe(false); // 제거
  expect(rows(s.root).map((r) => byClass(r, "ow-row__name")[0].textContent)).toEqual([
    "B동 101",
    "B동 103",
  ]); // 추가
  s.dispose();
});

test("1000번 갱신해도 노드 수가 늘지 않는다", () => {
  const s = setup(list([office("101"), office("102")]));
  const before = count(s.root);
  for (let i = 0; i < 1000; i++) {
    s.store.applyList(
      list(
        i % 2 ? [office("101"), office("102")] : [office("102", { dot: "Waiting" }), office("101")],
      ),
    );
  }
  expect(count(s.root)).toBe(before);
  s.dispose();
});

test("규모가 바뀌면 구조가 바뀐다: 동 머리글 목록에서 층 3개 고정 행으로", () => {
  const s = setup(list([office("101")]));
  expect(byClass(s.root, "ow-group").length).toBe(1);
  expect(s.root.dataset.scale).toBe("large");
  const oldRow = rows(s.root)[0];

  const floor2 = office("fl2", { floor: 2, building: null, label: "2층 Codex 사무실" });
  s.store.applyList(list([floor2], "Small"));
  expect(s.root.dataset.scale).toBe("small");
  expect(byClass(s.root, "ow-group").length).toBe(0);
  const rs = rows(s.root);
  expect(rs.filter((r) => r.classList.contains("is-floor")).length).toBe(3);
  expect(rs).not.toContain(oldRow);
  // 팀 행은 층 행 아래, 비어 있는 층은 공실
  expect(byClass(s.root, "is-vacant").length).toBe(2);
  expect(s.listEl.textContent).toContain("공실");
  expect(rowByName(s.root, "김하늘 부장 팀")).toBeTruthy();
  s.dispose();
});

test("빈 목록이면 안내 문구를 보인다", () => {
  const s = setup(list([]));
  expect(s.listEl.textContent).toContain("불 켜진 사무실이 없어요");
  expect(rows(s.root).length).toBe(0);
  s.store.applyList(list([office("101")]));
  expect(s.listEl.textContent).not.toContain("불 켜진 사무실이 없어요");
  s.dispose();
});

test("간판은 넘겨받은 회사 이름, 없으면 기본 이름이다", () => {
  const store = createStore<OfficeListDto, OfficeDto>();
  const name = (opts: SidebarOptions) => {
    const root = document.createElement("div");
    const dispose = mountSidebar(root, store, () => {}, { getWidth: () => 1200, ...opts });
    const text = byClass(root, "ow-brand__name")[0].textContent;
    dispose();
    return text;
  };
  expect(name({ companyName: "야근상사" })).toBe("야근상사");
  expect(name({})).toBe("Owlfice");
});

test("지표는 활성 팀이 있는 사무실·직원·결재 인원을 세고 퇴근 팀은 제외한다", () => {
  const offices = [
    office("101", {
      teams: [
        team("one", { others: 2, asking: true, askingCount: 2 }),
        team("off", { phase: "Off", others: 8, asking: true, askingCount: 5 }),
      ],
    }),
    office("102", { teams: [team("two", { others: 0 })] }),
    office("103", {
      phase: "Off",
      teams: [team("gone", { phase: "Off", others: 7, askingCount: 3 })],
    }),
    office("empty", { teams: [] }),
  ];
  const s = setup(list(offices));
  const values = () => byClass(s.root, "ow-kpi__v").map((v) => v.textContent);
  expect(values()).toEqual(["2", "4", "2"]);
  expect(byClass(s.root, "ow-kpi__k").map((v) => v.textContent)).toEqual([
    "사무실",
    "근무 중",
    "결재 대기",
  ]);
  expect(s.root.querySelector(".ow-kpi__v.is-ask")?.textContent).toBe("2");
  expect(s.root.querySelector(".ow-brand__tile .ow-brand__logo")).not.toBeNull();
  expect(s.root.querySelector(".ow-brand__sub")?.textContent).toBe("Owlfice · 대기업");
  s.store.applyList(list([]));
  expect(values()).toEqual(["0", "0", "0"]);
  expect(s.root.querySelector(".ow-kpi__v.is-ask")).toBeNull();
  s.dispose();
});

test("중소기업 지표는 활성 층 수와 모든 활성 팀의 직원·결재 인원을 합한다", () => {
  const s = setup(
    list(
      [
        office("claude", {
          floor: 1,
          teams: [
            team("one", { others: 2, askingCount: 2 }),
            team("two", { others: 1, askingCount: 1 }),
          ],
        }),
        office("codex", { floor: 2, teams: [team("three", { others: 0 })] }),
        office("gemini", { floor: 3, teams: [team("off", { phase: "Off", askingCount: 3 })] }),
      ],
      "Small",
    ),
  );
  expect(byClass(s.root, "ow-kpi__v").map((v) => v.textContent)).toEqual(["2", "6", "3"]);
  expect(byClass(s.root, "ow-kpi__k")[0].textContent).toBe("층");
  expect(s.root.querySelector(".ow-brand__sub")?.textContent).toBe("Owlfice · 중소기업");
  s.dispose();
});

test("레일은 동 색 타일이고 결재 시 타일 전체가 바뀌며 해제하면 복구된다", () => {
  const s = setup(list([office("101", { asking: true })]), 800);
  const tile = byClass(s.root, "ow-rail__item")[0];
  expect(tile.style.getPropertyValue("--ow-square")).toBe("var(--ow-ask)");
  expect(tile.textContent).toBe("B");
  expect(tile.querySelector(".ow-rail__letter")).toBeNull();
  expect(tile.querySelector<HTMLElement>(".ow-rail__state")!.dataset.state).toBe("ask");
  s.store.applyList(list([office("101")]));
  expect(tile.style.getPropertyValue("--ow-square")).toBe("var(--ow-claude)");
  s.dispose();
});

test("행을 누르면 store 선택과 select_office 를 부른다", () => {
  const s = setup(list([office("101"), office("102")]));
  byClass(rowByName(s.root, "B동 102"), "ow-row__main")[0].click();
  expect(s.store.state.selectedId).toBe("102");
  expect(s.picked).toEqual([["102", 1]]);
  expect(
    byClass(rowByName(s.root, "B동 102"), "ow-row__main")[0].getAttribute("aria-current"),
  ).toBe("true");
  // 이미 고른 사무실을 다시 눌러도 요청을 또 보내지 않는다
  byClass(rowByName(s.root, "B동 102"), "ow-row__main")[0].click();
  expect(s.picked.length).toBe(1);
  s.dispose();
});

test("간판 설정 열기·회사 이름·원문 경고·재탐색을 연결한다", async () => {
  const store = createStore<OfficeListDto, OfficeDto>();
  store.applyWorld({ ...list([]), selected: null });
  const root = document.createElement("div");
  document.body.append(root);
  const onSettings = vi.fn();
  const onRescan = vi.fn().mockRejectedValue(new Error("탐색 실패"));
  const dispose = mountSidebar(root, store, vi.fn(), { onSettings, onRescan });
  const brand = root.querySelector<HTMLButtonElement>(".ow-brand")!;
  brand.click();
  expect(onSettings).toHaveBeenLastCalledWith(brand);
  dispose.setSettings("새 회사", true);
  expect(root.querySelector(".ow-brand__name")!.textContent).toBe("새 회사");
  expect(root.querySelector<HTMLElement>(".ow-brand__warn")!.hidden).toBe(false);
  const refresh = root.querySelector<HTMLButtonElement>('[aria-label="지금 다시 찾기"]')!;
  refresh.click();
  refresh.click();
  expect(onRescan).toHaveBeenCalledTimes(1);
  await Promise.resolve();
  await Promise.resolve();
  expect(root.querySelector('[role="alert"]')!.textContent).toContain("탐색 실패");
  expect(refresh.disabled).toBe(false);
  dispose.setSettings(null, false);
  expect(root.querySelector(".ow-brand__name")!.textContent).toBe("Owlfice");
  expect(root.querySelector<HTMLElement>(".ow-brand__warn")!.hidden).toBe(true);
  dispose();
});

test("요청 중 같은 사무실은 다시 부르지 않고 복구 실패 뒤에는 재선택한다", async () => {
  const s = setup(list([office("101"), office("102")]));
  const row = byClass(rowByName(s.root, "B동 102"), "ow-row__main")[0];
  row.click();
  row.click();
  expect(s.picked).toHaveLength(1);
  const log = vi.spyOn(console, "error").mockImplementation(() => {});
  await s.store.requestSelect(s.picked[0][1], Promise.reject(new Error("선택 실패")), async () => {
    row.click();
    expect(s.picked).toHaveLength(1);
    throw new Error("복구 실패");
  });
  log.mockRestore();
  row.click();
  expect(s.picked).toHaveLength(2);
  expect(s.picked[1]).toEqual(["102", s.picked[0][1] + 1]);
  row.click();
  expect(s.picked).toHaveLength(2);
  s.dispose();
});

test("중소기업: 팀 행을 누르면 그 층 사무실을 고른다", () => {
  const floor = office("fl1", { floor: 1, building: null, label: "1층 Claude 사무실" });
  const s = setup(list([floor], "Small"));
  byClass(rowByName(s.root, "김하늘 부장 팀"), "ow-row__main")[0].click();
  expect(s.picked).toEqual([["fl1", 1]]);
  s.dispose();
});

test("퇴근 묶음은 접혀 있고 머리글을 누르면 펼쳐진다", () => {
  const s = setup(
    list([
      office("101"),
      office("102", { phase: "Off", dot: "Off", teams: [team("x", { phase: "Off" })] }),
    ]),
  );
  const off = rowByName(s.root, "B동 102");
  expect(off.hidden).toBe(true);
  expect(off.classList.contains("is-off")).toBe(true);
  byClass(s.root, "ow-group").slice(-1)[0].click();
  expect(off.hidden).toBe(false);
  expect(rowByName(s.root, "B동 102")).toBe(off);
  s.dispose();
});

test("창 너비 구간: 1600·960 사이드바, 800 레일, 720 미만 드롭다운, 디바운스 100ms", () => {
  const s = setup(list([office("101"), office("102")]), 1600);
  expect(s.root.dataset.mode).toBe("full");

  s.widthRef.w = 960;
  roInstances[0].cb();
  vi.advanceTimersByTime(100);
  expect(s.root.dataset.mode).toBe("full");

  s.widthRef.w = 800;
  roInstances[0].cb();
  vi.advanceTimersByTime(99);
  expect(s.root.dataset.mode).toBe("full"); // 아직 디바운스 중
  vi.advanceTimersByTime(1);
  expect(s.root.dataset.mode).toBe("rail");
  expect(byClass(s.root, "ow-rail__item").length).toBe(2);

  // 레일에 올리면 240 이 겹쳐 열리고, 벗어나면 닫힌다
  fire(byClass(s.root, "ow-rail")[0], "mouseenter");
  expect(s.root.classList.contains("is-pop")).toBe(true);
  fire(s.root, "mouseleave");
  expect(s.root.classList.contains("is-pop")).toBe(false);

  s.widthRef.w = 719;
  roInstances[0].cb();
  vi.advanceTimersByTime(100);
  expect(s.root.dataset.mode).toBe("select");
  const opts = [...byClass(s.root, "ow-side-select")[0].children] as HTMLOptionElement[];
  expect(opts.map((o) => o.value)).toEqual(["101", "102"]);

  // 드롭다운으로 고르면 행 클릭과 같다
  const sel = byClass(s.root, "ow-side-select")[0] as HTMLSelectElement;
  sel.value = "102";
  fire(sel, "change");
  expect(s.picked).toEqual([["102", 1]]);

  s.widthRef.w = 900;
  roInstances[0].cb();
  vi.advanceTimersByTime(100);
  expect(s.root.dataset.mode).toBe("full");
  s.dispose();
});

test("토스트: 입주·퇴근·재출근·퇴거를 3초씩 차례로", () => {
  const off = office("102", { phase: "Off", dot: "Off", teams: [team("x", { phase: "Off" })] });
  const s = setup(list([office("101"), off, office("103")]));
  expect(s.slot.children.length).toBe(0); // 첫 목록은 알리지 않는다

  s.store.applyList(
    list([office("101", { phase: "Off", dot: "Off" }), office("102"), office("104")]),
  );
  // 신규 목록 순서(101 퇴근, 102 재출근, 104 입주) 뒤에 퇴거(103)
  const order = ["B동 101 퇴근", "B동 102 재출근", "B동 104 입주", "B동 103 퇴거"];
  for (const [i, want] of order.entries()) {
    expect(s.slot.children.length).toBe(1);
    expect(s.slot.textContent).toContain(want);
    vi.advanceTimersByTime(2999);
    expect(s.slot.children.length).toBe(1);
    vi.advanceTimersByTime(1);
    expect(s.slot.children.length).toBe(i === order.length - 1 ? 0 : 1);
  }
  s.dispose();
});

test("조용한 재구성이면 토스트가 없다", () => {
  const s = setup(list([office("101")]));
  s.store.applyList(list([office("101"), office("102")], "Large", { rebuilt: true }));
  expect(s.slot.children.length).toBe(0);
  s.dispose();
});

test("활성 경과는 1분마다 그 칸만 고친다", () => {
  const s = setup(list([office("101")]));
  const row = rows(s.root)[0];
  const elapsed = byClass(row, "ow-row__elapsed")[0];
  const head = byClass(row, "ow-row__head")[0];
  expect(head.textContent).toBe("lumaweave · 부장 외 3명 · ");
  expect(elapsed.textContent).toBe("12분째");
  vi.advanceTimersByTime(60_000);
  expect(elapsed.textContent).toBe("13분째");
  s.dispose();
});

test("중소기업: 새 팀이 들어오면 팀 출근 토스트", () => {
  const floor = (teams: TeamSummaryDto[]) =>
    office("fl1", { floor: 1, building: null, label: "1층 Claude 사무실", teams });
  const s = setup(list([floor([team("a")])], "Small"));
  s.store.applyList(list([floor([team("a"), team("b", { bossName: "박민준" })])], "Small"));
  expect(s.slot.textContent).toContain("박민준 부장 팀 출근");
  s.dispose();
});

test("규모를 바꾼 목록에선 토스트가 없다", () => {
  const s = setup(list([office("101")]));
  s.store.applyList(
    list([office("fl1", { floor: 1, building: null, label: "1층 Claude 사무실" })], "Small"),
  );
  expect(s.slot.children.length).toBe(0);
  s.dispose();
});

test("토스트 칸은 aside 밖에 있어 레일 모드에서도 남는다", () => {
  const s = setup(list([office("101")]), 850);
  expect(s.slot.closest("aside")).toBeNull();
  expect(s.slot.getAttribute("role")).toBe("status");
  s.dispose();
});

test("백엔드가 선택을 바꾸면 aria-current 가 따라간다", () => {
  const s = setup(list([office("101"), office("102")]));
  s.store.applyList(list([office("101"), office("102")], "Large", { selectedId: "102" }));
  const main = (name: string) => byClass(rowByName(s.root, name), "ow-row__main")[0];
  expect(main("B동 102").getAttribute("aria-current")).toBe("true");
  expect(main("B동 101").hasAttribute("aria-current")).toBe(false);
  expect(rowByName(s.root, "B동 102").hasAttribute("aria-current")).toBe(false);
  s.dispose();
});

test("행 버튼 이름에 이름·상태·업무·폴더·경과가 들어간다", () => {
  const s = setup(list([office("101", { asking: true })]));
  const main = byClass(rows(s.root)[0], "ow-row__main")[0];
  expect(main.getAttribute("aria-label")).toBe(
    "B동 101, 근무 중, 허가 대기, 버그 수정, lumaweave, 12분째",
  );
  s.dispose();
});

test("30분 조용·프로세스 있음: 대기 중 칩이 업무를 대신하고 깨어나면 업무를 복구한다", () => {
  const idle = (dot: Dot) =>
    office("101", { phase: "Dozing", dot, teams: [team("t-101", { phase: "Dozing", dot })] });
  const s = setup(list([idle("IdleAlive")]));
  const row = rowByName(s.root, "B동 101");
  const chip = byClass(row, "ow-worktype")[0];
  expect([dotOf(row).dataset.state, chip.hidden, chip.textContent]).toEqual([
    "idle-alive",
    false,
    "대기 중",
  ]);
  expect(chip.classList.contains("is-idle")).toBe(true);
  expect(byClass(row, "ow-row__main")[0].getAttribute("aria-label")).toContain("대기 중");
  s.store.applyList(list([idle("Dozing")]));
  expect([dotOf(row).dataset.state, chip.textContent]).toEqual(["sleepy", "버그 수정"]);
  expect(chip.classList.contains("is-idle")).toBe(false);
  // 중소기업 팀 행도 같다
  s.store.applyList(
    list(
      [office("vendor/claude", { floor: 1, teams: [team("t", { dot: "IdleAlive" })] })],
      "Small",
    ),
  );
  const teamRow = rowByName(s.root, "김하늘 부장 팀");
  expect(teamRow.querySelector(".ow-worktype.is-idle")?.textContent).toBe("대기 중");
  s.dispose();
});

test("로빙 tabindex: 정지점은 하나, ↑↓ 로 옮긴다", () => {
  const s = setup(list([office("101"), office("102"), office("103")]));
  const mains = rows(s.root).map((r) => byClass(r, "ow-row__main")[0]);
  expect(mains.map((m) => m.getAttribute("tabindex"))).toEqual(["0", "-1", "-1"]);
  const key = (el: Element, k: string) =>
    el.dispatchEvent(new KeyboardEvent("keydown", { key: k, bubbles: true, cancelable: true }));
  mains[0].focus();
  key(mains[0], "ArrowDown");
  expect(document.activeElement).toBe(mains[1]);
  expect(mains.map((m) => m.getAttribute("tabindex"))).toEqual(["-1", "0", "-1"]);
  key(mains[1], "ArrowUp");
  expect(document.activeElement).toBe(mains[0]);
  key(mains[0], "ArrowUp"); // 맨 위에서 멈춘다
  expect(document.activeElement).toBe(mains[0]);
  s.dispose();
});

test("레일 항목을 누르면 고르지 않고 오버레이를 열어 그 행으로 초점을 옮긴다", () => {
  const s = setup(list([office("101"), office("102")]), 850);
  const item = byClass(s.root, "ow-rail__item")[1];
  item.click();
  expect(s.picked).toEqual([]);
  expect(s.root.classList.contains("is-pop")).toBe(true);
  expect(document.activeElement).toBe(byClass(rowByName(s.root, "B동 102"), "ow-row__main")[0]);
  s.dispose();
});

test("공실 층은 행·레일·option 이 모두 고를 수 없고, 선택이 없으면 option 도 비어 있다", () => {
  const floor = office("fl2", { floor: 2, building: null, label: "2층 Codex 사무실" });
  const s = setup(list([floor], "Small"), 850);
  const disabledRows = rows(s.root).filter((r) =>
    byClass(r, "ow-row__main")[0].matches(":disabled"),
  );
  expect(disabledRows.length).toBe(2);
  const railItems = byClass(s.root, "ow-rail__item");
  expect(railItems.map((i) => i.hasAttribute("disabled"))).toEqual([true, false, true]);
  const sel = byClass(s.root, "ow-side-select")[0] as HTMLSelectElement;
  expect([...sel.options].map((o) => o.disabled)).toEqual([true, false, true]);
  expect(sel.selectedIndex).toBe(-1);
  s.dispose();
});

test("동이 없는 사무실은 머리글 없이 행만 둔다", () => {
  const s = setup(list([office("101", { building: null })]));
  expect(byClass(s.root, "ow-group").length).toBe(0);
  expect(rows(s.root).length).toBe(1);
  s.dispose();
});

test("dispose 하면 붙인 요소와 상태를 거둔다", () => {
  const s = setup(list([office("101")]));
  s.dispose();
  expect(s.root.children.length).toBe(0);
  expect(s.root.classList.contains("ow-side-host")).toBe(false);
});

test("빈 상태: 아이콘과 제목·본문이 보인다", () => {
  const el = emptyState(document, "제목", "본문");
  expect(el.className).toBe("ow-empty");
  expect(el.querySelector("svg")).not.toBeNull();
  expect(el.querySelector("b")!.textContent).toBe("제목");
  expect(el.textContent).toContain("본문");
});

test("상세 버튼은 그 행의 사무실·팀으로 세션 상세를 열고 행 선택은 바꾸지 않는다", () => {
  const onDetail = vi.fn();
  const s = setup(list([office("101"), office("102")]), 1200, onDetail);
  const row = rowByName(s.root, "B동 102");
  const info = byClass(row, "ow-row__info")[0];
  expect(info.getAttribute("aria-label")).toBe("B동 102 세션 상세");
  expect(info.getAttribute("tabindex")).toBe("-1");
  info.click();
  expect(onDetail).toHaveBeenCalledWith("102", "t-102", "B동 102", info);
  expect(s.picked).toEqual([]);
});

test("→ 키로도 연다. 중소기업은 팀 행만 열고 층 행에는 버튼이 없다", () => {
  const onDetail = vi.fn();
  const floor = office("vendor/claude", {
    building: null,
    floor: 1,
    room: null,
    label: "1층 Claude 사무실",
    teams: [team("ta"), team("tb", { bossName: "서아린" })],
  });
  const s = setup(list([floor], "Small"), 1200, onDetail);
  const rs = rows(s.root);
  expect(byClass(rs[0], "ow-row__info")[0].hidden).toBe(true); // 층 행
  const second = rs[2]; // 서아린 부장 팀
  const main = byClass(second, "ow-row__main")[0];
  main.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowRight", bubbles: true }));
  // 복귀 대상은 행 버튼이다: 아이콘이면 Esc 뒤 ↑↓ 로빙이 끊긴다
  expect(onDetail).toHaveBeenCalledWith("vendor/claude", "tb", "서아린 부장 팀", main);
  byClass(rs[0], "ow-row__main")[0].dispatchEvent(
    new KeyboardEvent("keydown", { key: "ArrowRight", bubbles: true }),
  );
  expect(onDetail).toHaveBeenCalledTimes(1);
});

test("→ 로 연 상세를 닫고 초점이 행 버튼으로 돌아오면 ↓ 가 다음 행으로 간다", () => {
  let opener: HTMLElement | undefined;
  const s = setup(list([office("101"), office("102")]), 1200, (_o, _t, _n, el) => {
    opener = el;
  });
  document.body.append(s.root);
  const rs = rows(s.root);
  const main = byClass(rs[0], "ow-row__main")[0];
  main.focus();
  main.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowRight", bubbles: true }));
  opener!.focus(); // popup-modal 이 닫을 때 하는 일
  expect(opener).toBe(main);
  opener!.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true }));
  expect(document.activeElement).toBe(byClass(rs[1], "ow-row__main")[0]);
});

test("팝업 이름은 클릭이 준 사무실·팀으로 찾는다 (선택과 달라도)", () => {
  const l = list([
    office("101"),
    office("102", { teams: [team("t-102", { bossName: "서아린" })] }),
  ]);
  expect(popupNames(l, "102", "t-102")).toEqual({
    officeName: "B동 102",
    teamLabel: "서아린 부장 팀",
  });
  expect(popupNames(l, "102", "없음").teamLabel).toBe("B동 102");
  expect(popupNames(l, "없음", "t").officeName).toBe("");
});
