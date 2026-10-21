// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, test, vi } from "vite-plus/test";
import { mountPanel, type PanelHandle, type PanelOptions, type Viewport } from "./sidebar-right.ts";
import { createStore } from "./store.ts";
import { act, emp, iso, office, team } from "./test-office.ts";
import type { OfficeDto, OfficeListDto } from "./types/ipc.ts";
import { setAvatar } from "./avatar.ts";

vi.mock("./avatar.ts", () => ({ setAvatar: vi.fn() }));

// 2026-01-01 14:32:00 (로컬)
const NOW = new Date(2026, 0, 1, 14, 32, 0);
const scrollIntoView = vi.fn();

beforeEach(() => {
  vi.useFakeTimers();
  vi.setSystemTime(NOW);
  Element.prototype.scrollIntoView = scrollIntoView;
  scrollIntoView.mockClear();
});
afterEach(() => {
  vi.useRealTimers();
  document.body.innerHTML = "";
});

function setup(
  first: OfficeDto,
  scale: "Large" | "Small" = "Large",
  size = { w: 1280, h: 800 },
  extra: Partial<PanelOptions> = {},
) {
  const store = createStore<OfficeListDto, OfficeDto>();
  store.applyWorld({
    listSeq: 1,
    rebuilt: false,
    scale,
    offices: [
      {
        officeId: first.officeId,
        building: "B",
        floor: 2,
        room: 203,
        label: "lumaweave",
        phase: "Working",
        dot: "Working",
        asking: false,
        border: "Plain",
        delayed: false,
        teams: [],
      },
    ],
    selectedId: first.officeId,
    selectRequest: 0,
    selected: first,
  });
  const cbs = new Set<() => void>();
  const vp: Viewport & { set(w: number, h: number): void } = {
    size: () => size,
    subscribe: (cb) => (cbs.add(cb), () => cbs.delete(cb)),
    set(w, h) {
      size = { w, h };
      cbs.forEach((cb) => cb());
    },
  };
  const root = document.createElement("div");
  document.body.append(root);
  const onDrawerChange = vi.fn();
  const panel: PanelHandle = mountPanel(root, store, { viewport: vp, onDrawerChange, ...extra });
  const q = <T extends Element = HTMLElement>(sel: string) => root.querySelector<T>(sel);
  const cards = () => [...root.querySelectorAll<HTMLElement>(".ow-card")];
  const ids = () => cards().map((c) => c.dataset.agentId);
  const push = (o: OfficeDto) => store.applyOfficeState(o);
  return { store, root, panel, vp, q, cards, ids, push, onDrawerChange };
}

const nodeCount = (el: Element): number => el.querySelectorAll("*").length + el.childNodes.length;

test("재사용 카드 아바타에 직원의 새 직급·담당과 팀 제조사를 전달한다", () => {
  const s = setup(office("o", 1, [team("t", [emp("a")], { building: "C" })]));
  const avatar = s.q<HTMLElement>(".ow-avatar")!;
  expect(setAvatar).toHaveBeenLastCalledWith(
    avatar,
    expect.objectContaining({ agentId: "a", rank: "Daeri" }),
    "C",
  );
  s.push(
    office("o", 2, [team("t", [emp("a", { rank: "Gwajang", role: "Qa" })], { building: "E" })]),
  );
  expect(s.q(".ow-avatar")).toBe(avatar);
  expect(setAvatar).toHaveBeenLastCalledWith(
    avatar,
    expect.objectContaining({ agentId: "a", rank: "Gwajang", role: "Qa" }),
    "E",
  );
  s.panel.destroy();
});

test("중소기업 현황판 팀 색은 책상 슬롯을 따르고 활동 재정렬에도 유지된다", () => {
  const teams = [
    team("first", [emp("boss")], { deskSlot: 0, lastActivity: iso(14, 1) }),
    team("second", [emp("boss")], { deskSlot: 1, lastActivity: iso(14, 2) }),
    team("spare", [emp("boss")], { deskSlot: 6, lastActivity: iso(14, 3) }),
    team("card", [emp("boss")], { lastActivity: iso(14, 4) }),
  ];
  const s = setup(office("vendor/Claude", 1, teams), "Small");
  const colors = () =>
    Object.fromEntries(
      [...s.root.querySelectorAll<HTMLElement>(".ow-teamhead")].map((head) => [
        head.textContent,
        head.querySelector<HTMLElement>(".ow-square")!.style.getPropertyValue("--ow-square"),
      ]),
    );
  const before = colors();
  expect(Object.values(before).sort()).toEqual([
    "var(--ow-team-1)",
    "var(--ow-team-2)",
    "var(--ow-team-6)",
    "var(--ow-team-6)",
  ]);
  teams[0].lastActivity = iso(14, 5);
  s.push(office("vendor/Claude", 2, [...teams].reverse()));
  expect(colors()).toEqual(before);
  s.panel.destroy();
});

test("중소기업 퇴근 팀은 접히고 팀 선택·재출근으로 다시 펼쳐진다", () => {
  const state = (seq: number, phase: "Working" | "Off") =>
    office("vendor/Claude", seq, [team("t", [emp("boss"), emp("sub")], { phase })]);
  const s = setup(state(1, "Working"), "Small");
  s.push(state(2, "Off"));
  const head = s.root.querySelector<HTMLButtonElement>(".ow-teamhead")!;
  expect(head.getAttribute("aria-expanded")).toBe("false");
  expect(s.cards().every((c) => c.hidden)).toBe(true);
  s.panel.showTeam("vendor/Claude", "t");
  expect(head.getAttribute("aria-expanded")).toBe("true");
  expect(s.cards().every((c) => !c.hidden)).toBe(true);
  expect(scrollIntoView.mock.contexts[scrollIntoView.mock.contexts.length - 1]).toBe(head);
  head.click();
  expect(s.cards().every((c) => c.hidden)).toBe(true);
  s.push(state(3, "Working"));
  expect(head.getAttribute("aria-expanded")).toBe("true");
  expect(s.cards().every((c) => !c.hidden)).toBe(true);
  s.panel.destroy();
});

test("팀 스크롤은 해당 사무실 응답까지 기다리고 다른 선택이 오면 버린다", () => {
  const s = setup(office("vendor/Claude", 1, [team("a", [emp("a")])]), "Small");
  s.store.selectOffice("vendor/Codex");
  s.panel.showTeam("vendor/Codex", "b");
  expect(scrollIntoView).not.toHaveBeenCalled();
  s.push(office("vendor/Codex", 1, [team("b", [emp("b")])]));
  expect(scrollIntoView).toHaveBeenCalledTimes(1);
  s.store.selectOffice("vendor/Claude");
  s.panel.showTeam("vendor/Claude", "a");
  s.store.selectOffice("vendor/Gemini");
  s.push(office("vendor/Gemini", 1, [team("c", [emp("c")])]));
  expect(scrollIntoView).toHaveBeenCalledTimes(1);
  s.panel.destroy();
});

describe("사무실 카드 전환", () => {
  const animate = vi.fn<HTMLElement["animate"]>();
  const cancel = vi.fn();
  const nextOffice = (id: string, seq = 1) => office(id, seq, [team(id, [emp(id)])]);

  beforeEach(() => {
    Object.defineProperty(HTMLElement.prototype, "animate", {
      configurable: true,
      value: animate.mockReset().mockReturnValue({ cancel } as unknown as Animation),
    });
    cancel.mockReset();
    vi.spyOn(document, "hidden", "get").mockReturnValue(false);
  });
  afterEach(() => {
    Reflect.deleteProperty(HTMLElement.prototype, "animate");
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
  });

  test("첫 로드는 생략하고 새 카드를 즉시 배치한 뒤 300ms 페이드한다", () => {
    const s = setup(nextOffice("A"));
    expect(animate).not.toHaveBeenCalled();
    s.store.selectOffice("B");
    s.push(nextOffice("B"));
    expect(s.ids()).toEqual(["B"]);
    expect(animate).toHaveBeenCalledExactlyOnceWith([{ opacity: 0 }, { opacity: 1 }], {
      duration: 300,
      easing: "ease",
    });
    s.push(nextOffice("B", 2));
    expect(animate).toHaveBeenCalledTimes(1);
    s.panel.destroy();
  });

  test("연속 선택은 이전 페이드를 취소하고 마지막 카드만 남긴다", () => {
    const s = setup(nextOffice("A"));
    s.store.selectOffice("B");
    s.push(nextOffice("B"));
    s.store.selectOffice("C");
    expect(cancel).toHaveBeenCalled();
    s.push(nextOffice("C"));
    expect(s.ids()).toEqual(["C"]);
    expect(animate).toHaveBeenCalledTimes(2);
    s.panel.destroy();
  });

  test("숨은 동안 받은 선택은 페이드 없이 배치하고 복귀해도 다시 재생하지 않는다", () => {
    const s = setup(nextOffice("A"));
    s.store.selectOffice("B");
    s.push(nextOffice("B"));
    vi.spyOn(document, "hidden", "get").mockReturnValue(true);
    document.dispatchEvent(new Event("visibilitychange"));
    expect(cancel).toHaveBeenCalled();
    animate.mockClear();
    s.store.selectOffice("C");
    s.push(nextOffice("C"));
    expect(s.ids()).toEqual(["C"]);
    vi.spyOn(document, "hidden", "get").mockReturnValue(false);
    document.dispatchEvent(new Event("visibilitychange"));
    s.push(nextOffice("C", 2));
    expect(animate).not.toHaveBeenCalled();
    s.panel.destroy();
  });

  test("움직임 줄이기에서는 새 카드를 즉시 보이고 페이드하지 않는다", () => {
    const media = {
      matches: true,
      addEventListener: vi.fn(),
      removeEventListener: vi.fn(),
    };
    vi.stubGlobal("matchMedia", () => media);
    const s = setup(nextOffice("A"));
    s.store.selectOffice("B");
    s.push(nextOffice("B"));
    expect(s.ids()).toEqual(["B"]);
    expect(animate).not.toHaveBeenCalled();
    s.panel.destroy();
    expect(media.removeEventListener).toHaveBeenCalledWith("change", expect.any(Function));
  });
});

test("같은 agent id 카드는 다시 만들지 않고 바뀐 칸만 고친다", () => {
  const o = (seq: number, sentence: string) =>
    office("o", seq, [team("t", [emp("boss"), emp("a", { activity: act({ sentence }) })])]);
  const s = setup(o(1, "처음"));
  const before = s.cards();
  const nameNode = before[1].querySelector(".ow-card__name");
  s.push(o(2, "나중"));
  expect(s.cards()).toEqual(before);
  expect(s.cards()[1]).toBe(before[1]);
  expect(s.cards()[1].querySelector(".ow-now__text")?.textContent).toBe("나중");
  expect(s.cards()[1].querySelector(".ow-card__name")).toBe(nameNode);
});

test("1000번 갱신해도 목록 노드 수가 그대로고 카드는 같은 요소다", () => {
  const o = (i: number) =>
    office("o", i, [
      team("t", [
        emp("boss"),
        emp("a", { activity: act({ sentence: `작업 ${i}`, since: iso(14, i % 30) }) }),
        emp("b", { counters: { read: i, edit: 0, shell: 0, outside: 0, meeting: 0, errors: 0 } }),
      ]),
    ]);
  const s = setup(o(1));
  const list = s.q(".ow-panel__list")!;
  const cards = s.cards();
  const n = nodeCount(list);
  for (let i = 2; i <= 1001; i++) s.push(o(i));
  expect(nodeCount(list)).toBe(n);
  expect(s.cards().length).toBe(cards.length);
  expect(new Set(s.cards())).toEqual(new Set(cards));
});

test("정렬: 부장 맨 위, 최근 사건 순, 탕비실 맨 아래. 순서만 바뀌고 요소는 그대로", () => {
  const o = (seq: number, a: number, b: number) =>
    office("o", seq, [
      team("t", [
        emp("boss"),
        emp("a", { activity: act({ since: iso(14, a) }) }),
        emp("b", { activity: act({ since: iso(14, b) }) }),
        emp("p", { activity: act({ room: "Pantry", since: iso(14, 59) }) }),
      ]),
    ]);
  const s = setup(o(1, 1, 2));
  expect(s.ids()).toEqual(["boss", "b", "a", "p"]);
  const before = s.cards();
  s.push(o(2, 5, 2));
  expect(s.ids()).toEqual(["boss", "a", "b", "p"]);
  expect(new Set(s.cards())).toEqual(new Set(before));
  expect(s.cards()[0].classList.contains("is-boss")).toBe(true);
  expect(s.cards()[3].classList.contains("is-idle")).toBe(true);
});

test("추정은 점선 칩과 추정 꼬리표", () => {
  const o = (seq: number, c: "Confirmed" | "Inferred") =>
    office("o", seq, [team("t", [emp("boss", { activity: act({ confidence: c }) })])]);
  const s = setup(o(1, "Inferred"));
  const chip = s.q(".ow-status")!;
  const tag = chip.querySelector<HTMLElement>("small")!;
  expect(chip.classList.contains("is-estimate")).toBe(true);
  expect(tag.hidden).toBe(false);
  expect(tag.textContent).toBe("추정");
  s.push(o(2, "Confirmed"));
  expect(chip.classList.contains("is-estimate")).toBe(false);
  expect(tag.hidden).toBe(true);
});

test("{elapsed} 는 1초마다 채우고 코드 조각 안은 그대로, 평균을 넘으면 길어지네", () => {
  const run = (sec: number) =>
    office("o", 1, [
      team("t", [
        emp("boss", {
          activity: act({
            sentence: "`cargo {elapsed}` 돌리는 중 · {elapsed}째 (평소 40초)",
            since: new Date(NOW.getTime() - sec * 1000).toISOString(),
            expectedSecs: 40,
            elapsed: true,
          }),
        }),
      ]),
    ]);
  const s = setup(run(34));
  const text = s.q(".ow-now__text")!;
  expect(text.textContent).toBe("cargo {elapsed} 돌리는 중 · 34초째 (평소 40초)");
  expect(s.q(".ow-progress")!.classList.contains("is-over")).toBe(false);
  expect(s.q(".ow-code")?.textContent).toBe("cargo {elapsed}");
  vi.advanceTimersByTime(7000);
  expect(text.textContent).toBe("cargo {elapsed} 돌리는 중 · 41초째 (평소 40초)");
  expect(s.q(".ow-progress")!.classList.contains("is-over")).toBe(true);
  expect(s.q<HTMLElement>(".ow-progress__long")!.hidden).toBe(false);
});

test("진행 캡션은 비율이 같아도 매초 바뀌고 평균을 넘긴 뒤에도 바뀐다", () => {
  const s = setup(
    office("o", 1, [
      team("t", [
        emp("boss", {
          activity: act({
            since: NOW.toISOString(),
            expectedSecs: 600,
          }),
        }),
      ]),
    ]),
  );
  const caption = s.q(".ow-progress__caption span")!;
  expect(caption.textContent).toBe("0초 / 평균 600초");
  vi.advanceTimersByTime(1000);
  expect(caption.textContent).toBe("1초 / 평균 600초");
  vi.advanceTimersByTime(700_000);
  expect(caption.textContent).toBe("701초 / 평균 600초");
  vi.advanceTimersByTime(1000);
  expect(caption.textContent).toBe("702초 / 평균 600초");
});

test("행적에서 지금 활동과 같은 줄은 뺀다", () => {
  const now = act({ sentence: "빌드 끝", since: iso(14, 30) });
  const s = setup(
    office("o", 1, [
      team("t", [
        emp("boss", {
          activity: now,
          recent: [now, act({ sentence: "앞 일", since: iso(14, 20) })],
        }),
      ]),
    ]),
  );
  const rows = [...s.root.querySelectorAll(".ow-trail__row:not([hidden]) .ow-trail__text")];
  expect(rows.map((r) => r.textContent)).toEqual(["앞 일"]);
});

test("평균이 없으면 막대가 없다", () => {
  const s = setup(office("o", 1, [team("t", [emp("boss")])]));
  expect(s.q<HTMLElement>(".ow-progress")!.hidden).toBe(true);
});

test("컨텍스트 눈금: 켜진 칸, 80% 이상 주황, 창 크기를 모르면 ?", () => {
  const ctx = (used: number | null, win: number | null) =>
    office("o", 1, [team("t", [emp("boss", { contextUsed: used, contextWindow: win })])]);
  const lit = (s: ReturnType<typeof setup>) => s.root.querySelectorAll(".ow-ctx i.is-on").length;
  let s = setup(ctx(62_000, 100_000));
  expect(lit(s)).toBe(3);
  expect(s.q(".ow-ctx")!.classList.contains("is-high")).toBe(false);
  expect(s.q(".ow-ctx b")!.textContent).toBe("62%");
  s = setup(ctx(90_000, 100_000));
  expect(lit(s)).toBe(5);
  expect(s.q(".ow-ctx")!.classList.contains("is-high")).toBe(true);
  s = setup(ctx(100_000, null));
  expect(lit(s)).toBe(3);
  expect(s.q(".ow-ctx b")!.textContent).toBe("50%?");
});

test("사고 cue 는 1초 흔들림, 반려 cue 는 결재판 1초, 같은 seq 는 다시 하지 않는다", () => {
  const base = [team("t", [emp("boss"), emp("a")])];
  const s = setup(office("o", 1, base));
  const a = s.cards()[1];
  const reject = a.querySelector<HTMLElement>(".ow-card__reject")!;
  expect(reject.hidden).toBe(true);
  const cued = office("o", 2, base, [
    { teamId: "t", kind: "Shake", agent: "a" },
    { teamId: "t", kind: "Reject", agent: "a" },
  ]);
  s.push(cued);
  expect(a.classList.contains("is-hit")).toBe(true);
  expect(reject.hidden).toBe(false);
  vi.advanceTimersByTime(999);
  expect(a.classList.contains("is-hit")).toBe(true);
  vi.advanceTimersByTime(1);
  expect(a.classList.contains("is-hit")).toBe(false);
  expect(reject.hidden).toBe(true);
  s.push(cued);
  expect(a.classList.contains("is-hit")).toBe(false);
});

test("새 직원은 is-new 와 입사 토스트, 첫 그리기에는 없다", () => {
  const o = (seq: number, withNew: boolean) =>
    office("o", seq, [team("t", [emp("boss"), ...(withNew ? [emp("n", { name: "신입" })] : [])])]);
  const s = setup(o(1, false));
  expect(s.root.querySelector(".ow-panel__toast")).toBeNull();
  s.push(o(2, true));
  const card = s.cards().find((c) => c.dataset.agentId === "n")!;
  expect(card.classList.contains("is-new")).toBe(true);
  expect(s.q(".ow-panel__toast")!.textContent).toBe("신입 대리 입사했습니다!");
  vi.advanceTimersByTime(3000);
  expect(s.q(".ow-panel__toast")).toBeNull();
  expect(card.classList.contains("is-new")).toBe(false);
});

test("중소기업은 팀 머리글, 마지막 사건이 최근인 팀이 위", () => {
  const o = office("o", 1, [
    team("old", [emp("b1")], { lastActivity: iso(13, 0) }),
    team("new", [emp("b2")], { lastActivity: iso(14, 0), hostBadge: "WSL" }),
  ]);
  const s = setup(o, "Small");
  const kids = [...s.q(".ow-panel__list")!.children];
  expect(kids.map((k) => k.className.split(" ")[0])).toEqual([
    "ow-teamhead",
    "ow-card",
    "ow-teamhead",
    "ow-card",
  ]);
  expect(kids[0].textContent).toContain("이름b2 부장 팀");
  expect(kids[0].textContent).toContain("folder-new");
  expect(kids[0].textContent).toContain("WSL");
  expect(kids[0].textContent).toContain("1시간 32분째");
  const l = setup(o, "Large");
  expect(l.root.querySelector(".ow-teamhead")).toBeNull();
});

test("두 팀(다른 호스트)에 같은 agent id 가 있어도 카드는 따로, cue 는 그 팀 카드에만", () => {
  const teams = [
    team("win", [emp("boss", { name: "윈부장" }), emp("a", { name: "윈대리" })], {
      lastActivity: iso(13, 0),
    }),
    team("wsl", [emp("boss", { name: "리부장" }), emp("a", { name: "리대리" })], {
      lastActivity: iso(14, 0),
      hostBadge: "WSL",
    }),
  ];
  const s = setup(office("o", 1, teams), "Small");
  const names = () => s.cards().map((c) => c.querySelector(".ow-card__name")!.textContent);
  expect(names()).toEqual(["리부장", "리대리", "윈부장", "윈대리"]);
  s.push(office("o", 2, teams, [{ teamId: "wsl", kind: "Shake", agent: "a" }]));
  const hit = s.cards().filter((c) => c.classList.contains("is-hit"));
  expect(hit.map((c) => c.querySelector(".ow-card__name")!.textContent)).toEqual(["리대리"]);
  // 한 팀이 빠지면 그 팀 카드만 지운다
  const kept = s.cards()[3];
  s.push(office("o", 3, [teams[0]]));
  expect(names()).toEqual(["윈부장", "윈대리"]);
  expect(s.cards()[1]).toBe(kept);
});

test("store 가 선택을 바꿔 office 를 비우면 카드를 지운다", () => {
  const s = setup(office("o", 1, [team("t", [emp("boss")])]));
  expect(s.cards()).toHaveLength(1);
  s.store.selectOffice("other");
  expect(s.cards()).toHaveLength(0);
});

test("너비 1100 미만은 드로어, 높이 640 미만은 압축형과 티커 숨김", () => {
  const s = setup(office("o", 1, [team("t", [emp("boss")])]));
  const aside = s.q(".ow-panel")!;
  const ticker = s.q(".ow-ticker")!;
  expect(aside.hidden).toBe(false);
  expect(ticker.hidden).toBe(false);

  s.vp.set(1000, 800);
  expect(aside.hidden).toBe(true);
  s.panel.openDrawer();
  expect(aside.hidden).toBe(false);
  expect(aside.classList.contains("is-drawer")).toBe(true);
  expect(s.onDrawerChange).toHaveBeenLastCalledWith(true);
  s.q<HTMLElement>(".ow-panel__close")!.click();
  expect(aside.hidden).toBe(true);

  s.vp.set(1280, 600);
  expect(aside.hidden).toBe(false);
  expect(aside.classList.contains("is-drawer")).toBe(false);
  expect(ticker.hidden).toBe(true);
  expect(s.cards()[0].classList.contains("is-compact")).toBe(true);
  s.vp.set(1280, 800);
  expect(s.cards()[0].classList.contains("is-compact")).toBe(false);
});

test("티커 항목을 누르면 카드로 스크롤하고 강조한다", () => {
  const o = (seq: number, since: string) =>
    office("o", seq, [
      team("t", [
        emp("boss"),
        emp("a", { name: "서준", recent: [act({ sentence: "`lib.rs` 수정 완료", since })] }),
      ]),
    ]);
  const s = setup(o(1, iso(14, 31)));
  const item = s.q<HTMLElement>(".ow-ticker__item")!;
  expect(item.textContent).toContain("14:31");
  expect(item.textContent).toContain("서준 대리, lib.rs 수정 완료");
  item.click();
  const card = s.cards()[1];
  expect(scrollIntoView).toHaveBeenCalled();
  expect(card.classList.contains("is-highlight")).toBe(true);
  vi.advanceTimersByTime(1999);
  expect(card.classList.contains("is-highlight")).toBe(true);
  vi.advanceTimersByTime(1);
  expect(card.classList.contains("is-highlight")).toBe(false);
});

test("조용한 재구성과 첫 그리기에는 cue 도 입사 토스트도 없다", () => {
  const base = [team("t", [emp("boss"), emp("a")])];
  const cues: OfficeDto["cues"] = [{ teamId: "t", kind: "Shake", agent: "a" }];
  const s = setup({ ...office("o", 1, base, cues) });
  expect(s.cards()[1].classList.contains("is-hit")).toBe(false);
  const rebuilt = {
    ...office("o", 2, [team("t", [emp("boss"), emp("a"), emp("n")])], cues),
    rebuilt: true,
  };
  s.push(rebuilt);
  expect(s.cards()[1].classList.contains("is-hit")).toBe(false);
  expect(s.q(".ow-panel__toast")).toBeNull();
  expect(
    s
      .cards()
      .find((c) => c.dataset.agentId === "n")!
      .classList.contains("is-new"),
  ).toBe(false);
});

test("재정렬해도 초점은 그 카드에 남는다", () => {
  const o = (seq: number, a: number, b: number) =>
    office("o", seq, [
      team("t", [
        emp("boss"),
        emp("a", { activity: act({ since: iso(14, a) }) }),
        emp("b", { activity: act({ since: iso(14, b) }) }),
      ]),
    ]);
  const s = setup(o(1, 1, 2));
  const a = s.cards().find((c) => c.dataset.agentId === "a")!;
  a.focus();
  s.push(o(2, 5, 2));
  expect(s.ids()).toEqual(["boss", "a", "b"]);
  expect(document.activeElement).toBe(a);
});

test("카드에서 위아래 화살표는 이웃 카드로 초점을 옮긴다", () => {
  const s = setup(office("o", 1, [team("t", [emp("boss"), emp("a"), emp("b")])]));
  const [boss, a, b] = s.cards();
  const key = (el: HTMLElement, k: string) =>
    el.dispatchEvent(new KeyboardEvent("keydown", { key: k, bubbles: true, cancelable: true }));
  boss.focus();
  key(boss, "ArrowDown");
  expect(document.activeElement).toBe(a);
  key(a, "ArrowDown");
  expect(document.activeElement).toBe(b);
  key(b, "ArrowDown");
  expect(document.activeElement).toBe(b);
  key(b, "ArrowUp");
  expect(document.activeElement).toBe(a);
});

test("퇴근한 팀의 카드는 회색", () => {
  const o = (phase: "Working" | "Off") => office("o", 1, [team("t", [emp("boss")], { phase })]);
  expect(setup(o("Working")).cards()[0].classList.contains("is-idle")).toBe(false);
  expect(setup(o("Off")).cards()[0].classList.contains("is-idle")).toBe(true);
});

test("접근성: 반려 아이콘과 실적 칸에 이름이 있다", () => {
  const s = setup(office("o", 1, [team("t", [emp("boss")])]));
  const reject = s.q(".ow-card__reject")!;
  expect(reject.getAttribute("aria-hidden")).toBeNull();
  expect(reject.getAttribute("aria-label")).toBe("반려됨");
  expect(s.q(".ow-stats > span")!.getAttribute("aria-label")).toBe("읽은 파일 1");
});

test("카드를 누르면 (teamId, agentId) 와 그 카드로 직원 상세를 열고, highlight 는 2초 하이라이트", () => {
  const onEmployee = vi.fn();
  const o = office("o", 1, [
    team("t1", [emp("boss"), emp("a")]),
    team("t2", [emp("boss"), emp("a")]),
  ]);
  const s = setup(o, "Small", undefined, { onEmployee });
  // 같은 agentId 가 두 팀에 있어도 카드마다 자기 팀으로 부른다
  const cards = s.cards();
  expect(cards).toHaveLength(4);
  cards[3].click();
  expect(onEmployee).toHaveBeenLastCalledWith("t2", "a", cards[3]);
  cards[0].click();
  expect(onEmployee).toHaveBeenLastCalledWith("t1", "boss", cards[0]);

  s.panel.highlight("t2", "a");
  expect(cards[3].classList.contains("is-highlight")).toBe(true);
  expect(cards[1].classList.contains("is-highlight")).toBe(false);
  vi.advanceTimersByTime(2000);
  expect(cards[3].classList.contains("is-highlight")).toBe(false);
});

test("퇴근 서브는 '퇴근 n' 묶음에 접히고, 펼치면 60% 카드, 재출근하면 보통 자리로, 카드 클릭은 직원 상세", () => {
  const off = iso(14, 30);
  const o = (seq: number, back = false) =>
    office("o", seq, [
      team("t", [
        emp("boss"),
        emp("a", { departedAt: off }),
        emp("b", { departedAt: back ? null : off }),
        emp("c", { departedAt: off }),
        emp("p", { activity: act({ room: "Pantry", since: iso(14, 59) }) }),
      ]),
    ]);
  const onEmployee = vi.fn();
  const s = setup(o(1), "Large", undefined, { onEmployee });
  const btn = () => s.q<HTMLButtonElement>(".ow-group")!;
  const card = (id: string) => s.cards().find((c) => c.dataset.agentId === id)!;

  // (a) 탕비실 카드 아래에 "퇴근 3" 머리글, 카드는 접힘
  expect(s.ids()).toEqual(["boss", "p", "a", "b", "c"]);
  expect(btn().textContent).toContain("퇴근");
  expect(btn().querySelector(".ow-count")?.textContent).toBe("3");
  expect(btn().getAttribute("aria-expanded")).toBe("false");
  expect(["a", "b", "c"].map((id) => card(id).hidden)).toEqual([true, true, true]);
  expect(card("p").hidden).toBe(false);

  // (b) 펼침. 60% 클래스. 다시 그려도 안 접힘
  btn().click();
  expect(btn().getAttribute("aria-expanded")).toBe("true");
  expect(["a", "b", "c"].map((id) => card(id).hidden)).toEqual([false, false, false]);
  expect(card("a").classList.contains("is-departed")).toBe(true);
  s.push(o(2));
  expect(card("a").hidden).toBe(false);

  // (d) 카드 클릭은 직원 상세
  card("a").click();
  expect(onEmployee).toHaveBeenLastCalledWith("t", "a", card("a"));

  // (c) b 재출근: 묶음에서 나와 보통 자리, 머리글 개수 2
  s.push(o(3, true));
  expect(s.ids()).toEqual(["boss", "b", "p", "a", "c"]);
  expect(card("b").classList.contains("is-departed")).toBe(false);
  expect(btn().querySelector(".ow-count")?.textContent).toBe("2");
});
