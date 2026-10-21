// 직원 상세 팝업 (PLAN 3.7, HANDOFF 4.6). 요약은 메모리에서, 타임라인은 원본 파일을 50개씩 다시 읽어 온다.
import phrases from "../design/handoff/phrases.ko.json";
import chevronSvg from "../design/handoff/icons/chevron-right.svg?raw";
import editSvg from "../design/handoff/icons/edit.svg?raw";
import externalSvg from "../design/handoff/icons/open-external.svg?raw";
import fileSvg from "../design/handoff/icons/file.svg?raw";
import terminalSvg from "../design/handoff/icons/terminal.svg?raw";
import { detailErrorText, hms, openModal, smallIcon, type LiveEmployee } from "./popup-modal.ts";
import type { Invoke } from "./scale.ts";
import { CTX_CELLS, ROLE_LABEL, ROOM_LABEL, contextMeter, h, setRich } from "./sidebar-right.ts";
import { durationText, fill, hhmm } from "./text.ts";
import { rankLabel } from "./ticker.ts";
import { setAvatar } from "./avatar.ts";
import type {
  ClipDto,
  CountDto,
  CursorDto,
  EmployeeDetailDto,
  EventDto,
  OpenTarget,
  TimelineKind,
} from "./types/ipc.ts";

const D = phrases.employeeDetail;
/** 칩 이름. 도구 이름을 모르는 칸(결과 칸, 원본을 못 읽은 호출 칸)과 도구가 아닌 사건이 쓴다 */
const KIND: Record<TimelineKind, string> = phrases.timelineKind;
const PAGE = 50;

export interface EmployeePopupDeps {
  invoke: Invoke;
  live: LiveEmployee;
  doc?: Document;
  now?: () => number;
}
/** 직원은 (teamId, agentId) 로 가른다. 중소기업은 팀이 여럿이라 agentId 만으론 겹칠 수 있다 */
export interface EmployeeRef {
  officeId: string;
  teamId: string;
  agentId: string;
  /** 머리 줄에 쓸 사무실 이름 */
  officeName: string;
  building?: string;
}

const enc = new TextEncoder();
function sizeText(bytes: number): string {
  return bytes < 1024 ? `${bytes}B` : `${(bytes / 1024).toFixed(1)}KB`;
}
const lastName = (path: string): string => path.split(/[\\/]/).pop() ?? path;

export function openEmployeeDetail(
  deps: EmployeePopupDeps,
  ref: EmployeeRef,
  opener?: HTMLElement | null,
): void {
  const doc = deps.doc ?? document;
  const now = deps.now ?? Date.now;
  const { officeId, teamId, agentId } = ref;
  const m = openModal(doc, { width: 940, titleId: "ed-title", opener });

  // ----- 머리 -----
  const live = deps.live(teamId, agentId);
  const title = h(doc, "h1", "ow-pop__title ow-pop__title--lg", live?.name ?? "");
  title.id = "ed-title";
  const rank = h(doc, "span", "ow-pop__rank", live ? rankLabel(live.rank) : "");
  const chips = h(doc, "div", "ow-pop__chips");
  const meta = h(doc, "div", "ow-pop__meta");
  const head = h(doc, "header", "ow-pop__head ow-pop__head--emp");
  const avatar = h(doc, "div", "ow-avatar ow-avatar--lg");
  avatar.setAttribute("aria-hidden", "true");
  if (live) setAvatar(avatar, live, ref.building);
  head.append(
    avatar,
    h(doc, "div", "ow-pop__headmain", h(doc, "div", "ow-pop__titlerow", title, rank), chips, meta),
    m.closeBtn,
  );

  const body = h(doc, "div", "ow-modal__body ow-pop__body ow-emp");
  body.setAttribute("aria-live", "polite");
  m.modal.append(head, body);

  // ----- 타임라인 상태 -----
  let list: HTMLElement;
  let moreBtn: HTMLButtonElement;
  let notice: HTMLElement;
  let countEl: HTMLElement;
  let events = 0;
  let last: EventDto | null = null;

  const fail = (e: unknown): void => {
    const p = h(doc, "p", "ow-pop__error", detailErrorText(e));
    p.setAttribute("role", "alert");
    body.replaceChildren(p);
  };

  async function init(): Promise<void> {
    // 둘은 서로 모른다: 요약이 거절돼도 타임라인 호출은 이미 나갔다
    const detail = deps.invoke<EmployeeDetailDto>("get_employee_detail", {
      officeId,
      teamId,
      agentId,
    });
    const page = deps.invoke<EventDto[]>("get_employee_timeline", timelineArgs(null));
    // 거절 값을 잡아 두지 않으면 요약이 거절됐을 때 타임라인 쪽 거절이 처리되지 않은 채 남는다
    page.catch(() => {});
    let dto: EmployeeDetailDto;
    try {
      dto = await detail;
    } catch (e) {
      if (!m.closed) fail(e);
      return;
    }
    if (m.closed) return;
    build(dto);
    await receive(page, false);
  }

  const timelineArgs = (before: CursorDto | null) => ({
    officeId,
    teamId,
    agentId,
    before,
    limit: PAGE,
  });

  /** 한 페이지를 목록 끝에 붙인다. reset 이면 목록을 비우고 다시 쓴다 */
  async function receive(
    page: Promise<EventDto[]>,
    reset: boolean,
    message?: string,
  ): Promise<void> {
    let got: EventDto[];
    try {
      got = await page;
    } catch (e) {
      if (m.closed) return;
      moreBtn.disabled = false;
      if (String(e) === "cursor_invalid" && !reset) return restart();
      notice.textContent = detailErrorText(e);
      notice.hidden = false;
      return;
    }
    if (m.closed) return;
    if (reset) {
      list.replaceChildren();
      events = 0;
    }
    notice.textContent = message ?? "";
    notice.hidden = !message;
    for (const e of got) list.append(eventRow(e));
    events += got.length;
    last = got.length ? got[got.length - 1] : reset ? null : last;
    countEl.textContent = events <= PAGE ? fill(D.timelineCount, { n: events }) : String(events);
    // 50개를 채워 받았을 때만 더 있을 수 있다
    moreBtn.hidden = got.length < PAGE;
    moreBtn.disabled = false;
  }

  function restart(): Promise<void> {
    // last 는 성공한 응답에서만 바꾼다: 다시 불러오기마저 실패해도 남은 더 보기가 죽지 않게
    return receive(
      deps.invoke<EventDto[]>("get_employee_timeline", timelineArgs(null)),
      true,
      phrases.detailError.cursor_invalid,
    );
  }

  function more(): void {
    if (!last) return;
    moreBtn.disabled = true;
    void receive(
      deps.invoke<EventDto[]>("get_employee_timeline", timelineArgs(last.cursor)),
      false,
    );
  }

  // ----- 그리기 -----
  function build(dto: EmployeeDetailDto): void {
    setAvatar(avatar, dto, ref.building);
    title.textContent = dto.name;
    rank.textContent = rankLabel(dto.rank);
    fillHead(dto);

    const left = h(doc, "div", "ow-emp__main");
    if (dto.instruction)
      left.append(
        h(
          doc,
          "div",
          "ow-pop__block",
          h(doc, "h2", "ow-pop__h2", D.instruction),
          h(doc, "div", "ow-pop__box ow-pop__instruction", dto.instruction),
        ),
      );
    countEl = h(doc, "span", "ow-mono ow-pop__n");
    notice = h(doc, "p", "ow-pop__error");
    notice.setAttribute("role", "alert");
    notice.hidden = true;
    list = h(doc, "div", "ow-emp__list");
    moreBtn = h(doc, "button", "ow-pop__more", D.more);
    moreBtn.setAttribute("type", "button");
    moreBtn.append(h(doc, "span", "ow-mono", D.moreStep));
    moreBtn.hidden = true;
    moreBtn.addEventListener("click", more);
    left.append(
      h(
        doc,
        "div",
        "ow-pop__block",
        h(doc, "div", "ow-pop__tlhead", h(doc, "h2", "ow-pop__h2", D.timeline), countEl),
        notice,
        list,
        moreBtn,
      ),
    );
    body.replaceChildren(left, summary(dto));
  }

  function fillHead(dto: EmployeeDetailDto): void {
    const emp = deps.live(teamId, agentId);
    const chip = (t: string, cls = "") => h(doc, "span", `ow-chip ${cls}`, t);
    chips.replaceChildren(chip(fill(phrases.card.role, { role: ROLE_LABEL[dto.role] })));
    chips.append(chip(dto.model ?? phrases.card.modelUnknown, "ow-mono"));
    if (emp) {
      chips.append(chip(ROOM_LABEL[emp.activity.room]));
      const meter = contextMeter(emp.contextUsed, emp.contextWindow);
      if (meter) {
        const ctx = h(doc, "span", `ow-ctx${meter.high ? " is-high" : ""}`);
        for (let i = 0; i < CTX_CELLS; i++) ctx.append(h(doc, "i", i < meter.lit ? "is-on" : ""));
        ctx.append(h(doc, "b", "", meter.text));
        const c = chip("");
        c.title = meter.label;
        c.append(ctx);
        chips.append(c);
      }
    }
    const hired = Date.parse(dto.hiredAt);
    const sec = Number.isNaN(hired) ? 0 : Math.max(0, Math.floor((now() - hired) / 1000));
    // 상사가 없는(부장) 직원은 "상사 {parent}" 토막을 뺀다
    const text = D.meta
      .split(" · ")
      .filter((part) => dto.bossName !== null || !part.includes("{parent}"))
      .join(" · ");
    meta.textContent = fill(text, {
      joined: Number.isNaN(hired) ? "" : hhmm(hired),
      elapsed: durationText(sec, sec < 60),
      parent: dto.bossName ?? "",
      office: ref.officeName,
    });
  }

  function summary(dto: EmployeeDetailDto): HTMLElement {
    const aside = h(doc, "aside", "ow-emp__aside");
    aside.setAttribute("aria-label", D.summary);
    aside.append(h(doc, "h2", "ow-pop__h2 ow-pop__h2--lg", D.summary));
    const section = (label: string, svg: string, items: (string | CountDto)[]) => {
      aside.append(
        h(
          doc,
          "div",
          "ow-pop__block",
          h(
            doc,
            "div",
            "ow-emp__sumhead",
            smallIcon(doc, svg),
            h(doc, "h2", "ow-pop__h2", label),
            h(doc, "span", "ow-mono ow-pop__n", String(items.length)),
          ),
          ...items.map((x) => {
            if (typeof x === "string") {
              const li = h(doc, "div", "ow-emp__li", x);
              li.title = x;
              return li;
            }
            const li = h(
              doc,
              "div",
              "ow-emp__li ow-emp__li--count",
              h(doc, "span", "ow-emp__item", x.item),
              h(doc, "span", "ow-emp__times", fill(D.times, { n: x.times })),
            );
            li.title = x.item;
            return li;
          }),
        ),
      );
    };
    section(D.readFiles, fileSvg, dto.filesRead);
    section(D.editedFiles, editSvg, dto.filesEdited);
    section(D.commands, terminalSvg, dto.commands);
    return aside;
  }

  function result(e: EventDto): HTMLElement {
    const span = h(doc, "span", "ow-ev__res");
    const set = (cls: string, text: string) => {
      span.classList.add(cls);
      span.textContent = text;
    };
    switch (e.outcome) {
      case "Ok":
        set("is-ok", D.ok);
        break;
      case "Err":
      case "Rejected":
        set(
          "is-bad",
          e.test ? fill(D.failTests, { pass: e.test.passed, fail: e.test.failed }) : D.fail,
        );
        break;
      case "Started":
        // 타임라인은 Started 와 뒤의 Ok/Err 를 따로 남기는 이력이라 경과 시간을 붙이면 끝난 일도 진행 중으로 읽힌다
        set("is-run", D.started);
        break;
      case "Unknown":
        set("is-dim", D.unknown);
        break;
      default:
        break;
    }
    if (e.outcome && e.confidence === "Inferred")
      span.append(h(doc, "span", "ow-est-tag", phrases.card.estimate));
    return span;
  }

  function clipBlock(label: string | null, clip: ClipDto): HTMLElement {
    const box = h(doc, "div", "ow-pop__box");
    box.append(h(doc, "pre", "", clip.head));
    if (clip.tail) {
      const omitted = Math.max(
        0,
        clip.bytes - enc.encode(clip.head).length - enc.encode(clip.tail).length,
      );
      box.append(h(doc, "div", "ow-emp__cut", fill(D.cut, { size: sizeText(omitted) })));
      box.append(h(doc, "pre", "", clip.tail));
    }
    const block = h(doc, "div", "ow-pop__blk");
    if (label) block.append(h(doc, "span", "ow-pop__lab", label));
    block.append(box);
    return block;
  }

  function openLink(e: EventDto, kind: "SourceLine" | "Sidecar", file: string): HTMLElement {
    const btn = h(doc, "button", "ow-emp__link");
    btn.setAttribute("type", "button");
    btn.append(
      smallIcon(doc, externalSvg),
      D.openRaw,
      h(doc, "span", "ow-mono ow-emp__linkfile", file),
    );
    const err = h(doc, "p", "ow-pop__error");
    err.setAttribute("role", "alert");
    err.hidden = true;
    btn.addEventListener("click", async () => {
      err.hidden = true;
      const target: OpenTarget = {
        kind,
        officeId,
        teamId,
        agentId,
        cursor: e.cursor,
        lineIdx: e.lineIdx,
      };
      try {
        await deps.invoke("open_path", { target });
      } catch (x) {
        err.textContent = detailErrorText(x);
        err.hidden = false;
      }
    });
    const wrap = h(doc, "div", "ow-emp__open");
    wrap.append(btn, err);
    return wrap;
  }

  function eventRow(e: EventDto): HTMLElement {
    const row = h(doc, "div", "ow-ev");
    const headBtn = h(doc, "button", "ow-ev__head");
    headBtn.setAttribute("type", "button");
    headBtn.setAttribute("aria-expanded", "false");
    const sum = h(doc, "span", "ow-ev__sum");
    setRich(sum, e.summary);
    const chev = smallIcon(doc, chevronSvg);
    chev.classList.add("ow-ev__chev");
    headBtn.append(
      h(doc, "span", "ow-ev__t", hms(e.ts)),
      h(doc, "span", "ow-ev__tool", e.tool ?? KIND[e.kind]),
      sum,
      result(e),
      chev,
    );
    const detail = h(doc, "div", "ow-ev__body");
    detail.hidden = true;
    let built = false;
    headBtn.addEventListener("click", () => {
      const open = headBtn.getAttribute("aria-expanded") !== "true";
      headBtn.setAttribute("aria-expanded", String(open));
      detail.hidden = !open;
      if (open && !built) {
        built = true;
        if (e.input) detail.append(clipBlock(D.input, e.input));
        if (e.body) detail.append(clipBlock(e.kind === "ToolResult" ? D.result : null, e.body));
        detail.append(openLink(e, "SourceLine", `${lastName(e.path)}:${e.lineNo}`));
        if (e.sidecar) detail.append(openLink(e, "Sidecar", lastName(e.sidecar)));
      }
    });
    row.append(headBtn, detail);
    return row;
  }

  void init();
}
