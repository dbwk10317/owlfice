// 세션 상세 팝업 (docs/CONTRACTS.md C9, HANDOFF 4.6). 정보는 메모리에서, 최근 사건은 원본 줄을 다시 읽어 온다 (get_session_detail).
import phrases from "../design/handoff/phrases.ko.json";
import checkSvg from "../design/handoff/icons/check.svg?raw";
import copySvg from "../design/handoff/icons/copy.svg?raw";
import fileSvg from "../design/handoff/icons/file.svg?raw";
import folderSvg from "../design/handoff/icons/folder.svg?raw";
import { setAvatar } from "./avatar.ts";
import { detailErrorText, hms, openModal, smallIcon, type LiveEmployee } from "./popup-modal.ts";
import type { Invoke } from "./scale.ts";
import { h, icon, setRich } from "./sidebar-right.ts";
import { BUILDING_PRODUCT, WORK_TYPES } from "./sidebar-left.ts";
import { durationText, fill, hhmm } from "./text.ts";
import { rankLabel } from "./ticker.ts";
import type { EventDto, OpenTarget, SessionDetailDto } from "./types/ipc.ts";

const S = phrases.sessionDetail;
const FIRST = 20;
const MAX = 100;
const COPIED_MS = 1500;

export interface SessionPopupDeps {
  invoke: Invoke;
  live: LiveEmployee;
  doc?: Document;
  now?: () => number;
}
export interface SessionRef {
  officeId: string;
  teamId: string;
  /** 머리에 쓸 사무실(팀) 이름. DTO 에는 사무실 이름이 없다 */
  name: string;
}

export function openSessionDetail(
  deps: SessionPopupDeps,
  ref: SessionRef,
  opener?: HTMLElement | null,
): void {
  const doc = deps.doc ?? document;
  const now = deps.now ?? Date.now;
  const m = openModal(doc, { width: 780, titleId: "sd-title", opener });

  // ----- 머리 -----
  const square = h(doc, "span", "ow-header__square");
  const title = h(doc, "h1", "ow-pop__title", ref.name);
  title.id = "sd-title";
  const chip = h(doc, "span", "ow-chip");
  chip.hidden = true;
  const sub = h(doc, "p", "ow-pop__sub");
  const head = h(doc, "header", "ow-pop__head");
  head.append(
    h(doc, "div", "ow-pop__headmain", h(doc, "div", "ow-pop__titlerow", square, title, chip), sub),
    m.closeBtn,
  );

  const body = h(doc, "div", "ow-modal__body ow-pop__body");
  body.setAttribute("aria-live", "polite");

  // ----- 바닥 -----
  const note = h(doc, "p", "ow-pop__note");
  note.setAttribute("role", "alert");
  note.hidden = true;
  const open = async (target: OpenTarget, failText = detailErrorText): Promise<void> => {
    note.hidden = true;
    try {
      await deps.invoke("open_path", { target });
    } catch (e) {
      note.textContent = failText(e);
      note.hidden = false;
    }
  };
  const openBtn = (label: string, svg: string, target: OpenTarget) => {
    const b = h(doc, "button", "ow-btn");
    b.setAttribute("type", "button");
    b.append(icon(doc, svg), label);
    b.addEventListener("click", () => void open(target));
    return b;
  };
  const { officeId, teamId } = ref;
  const foot = h(
    doc,
    "footer",
    "ow-modal__foot",
    note,
    openBtn(S.openRaw, fileSvg, { kind: "SourceFolder", officeId, teamId }),
    openBtn(S.openWork, folderSvg, { kind: "WorkFolder", officeId, teamId }),
  );
  m.modal.append(head, body, foot);

  // ----- 불러오기 -----
  let timeline: HTMLElement | null = null;

  const fail = (e: unknown): void => {
    const p = h(doc, "p", "ow-pop__error", detailErrorText(e));
    p.setAttribute("role", "alert");
    body.replaceChildren(p);
  };

  async function load(limit: number): Promise<void> {
    let dto: SessionDetailDto;
    try {
      dto = await deps.invoke<SessionDetailDto>("get_session_detail", { officeId, teamId, limit });
    } catch (e) {
      if (m.closed) return;
      if (timeline) showTimelineError(e);
      else fail(e);
      return;
    }
    if (m.closed) return;
    // 더 보기가 원본을 못 읽으면 백엔드는 거절이 아니라 recentError 로 준다. 받아 둔 목록은 두고 오류만 보인다
    if (timeline && dto.recentError) return showTimelineError(dto.recentError);
    if (!timeline) first(dto);
    fillTimeline(dto, limit);
  }

  function first(dto: SessionDetailDto): void {
    const [product, key] = BUILDING_PRODUCT[dto.building] ?? ["", "codex"];
    square.style.setProperty("--ow-square", `var(--ow-${key})`);
    const work = phrases.workType[WORK_TYPES.indexOf(dto.workType)];
    chip.textContent = work ?? "";
    chip.hidden = !work;
    sub.textContent = [dto.building && `${dto.building}동`, product, dto.hostLabel]
      .filter(Boolean)
      .join(" · ");
    body.replaceChildren(infoTable(dto), (timeline = h(doc, "section", "ow-pop__timeline")));
  }

  function infoTable(dto: SessionDetailDto): HTMLElement {
    const dl = h(doc, "dl", "ow-kv");
    const kv = (label: string, value: string | Node, cls = ""): HTMLElement => {
      const dd = h(doc, "dd", cls);
      dd.append(value);
      const row = h(doc, "div", `ow-kv__item ${cls.includes("wide") ? "is-wide" : ""}`);
      row.append(h(doc, "dt", "", label), dd);
      dl.append(row);
      return dd;
    };
    const mono = (t: string) => h(doc, "span", "ow-mono ow-kv__mono", t);

    const idDd = kv(S.sessionId, mono(dto.sessionId));
    const copyBtn = h(doc, "button", "ow-iconbtn ow-iconbtn--sm");
    copyBtn.setAttribute("type", "button");
    copyBtn.setAttribute("aria-label", S.copyId);
    copyBtn.title = S.copyId;
    copyBtn.append(smallIcon(doc, copySvg));
    copyBtn.addEventListener("click", async () => {
      try {
        await navigator.clipboard.writeText(dto.sessionId);
      } catch (e) {
        console.error("세션 ID 복사", e);
        return;
      }
      copyBtn.replaceChildren(smallIcon(doc, checkSvg));
      setTimeout(() => copyBtn.replaceChildren(smallIcon(doc, copySvg)), COPIED_MS);
    });
    idDd.append(copyBtn);

    kv(S.branch, mono(dto.gitBranch ?? "—"));
    kv(S.folder, mono(dto.cwd), "is-wide");
    kv(S.model, mono(dto.model ?? phrases.card.modelUnknown));
    kv(S.startElapsed, startElapsed(dto.started));
    kv(S.turns, String(dto.turns));
    if (dto.tokens)
      kv(
        S.tokens,
        fill(S.tokensValue, {
          in: dto.tokens.input.toLocaleString("en-US"),
          out: dto.tokens.output.toLocaleString("en-US"),
        }),
      );
    const c = dto.headcount;
    kv(
      S.people,
      fill(S.peopleValue, {
        n: c.bujang + c.gwajang + c.daeri + c.sawon,
        b: c.bujang,
        g: c.gwajang,
        d: c.daeri,
        s: c.sawon,
      }),
    );
    if (dto.pr) {
      // 주소는 넘기지 않는다: 백엔드가 그 팀의 PR 주소를 검사해 연다
      const pr = h(doc, "button", "ow-kv__pr", `#${dto.pr.number} ${dto.pr.url}`);
      pr.setAttribute("type", "button");
      pr.title = dto.pr.url;
      pr.addEventListener(
        "click",
        // open_failed 문구는 탐색기 이야기라 브라우저로 여는 PR 에는 맞지 않는다
        () =>
          void open({ kind: "Pr", officeId, teamId }, (e) =>
            String(e) === "open_failed" ? S.prOpenFailed : detailErrorText(e),
          ),
      );
      kv(S.pr, pr);
    }
    return dl;
  }

  function startElapsed(startedIso: string): string {
    const at = Date.parse(startedIso);
    if (Number.isNaN(at)) return "";
    const day = new Date(at).toLocaleDateString("ko-KR", { month: "long", day: "numeric" });
    const sec = Math.max(0, Math.floor((now() - at) / 1000));
    const elapsed = fill(phrases.units.elapsed, { value: durationText(sec, sec < 60) });
    return `${day} ${hhmm(at)} · ${elapsed}`;
  }

  function who(e: EventDto): string {
    if (e.kind === "UserMessage") return phrases.ranks.boss;
    const emp = deps.live(teamId, e.agentId);
    return emp ? `${emp.name} ${rankLabel(emp.rank)}` : e.agentId;
  }

  function eventRow(e: EventDto, building: string): HTMLElement {
    const avatar = h(doc, "span", "ow-avatar ow-avatar--sm");
    const employee = e.kind === "UserMessage" ? undefined : deps.live(teamId, e.agentId);
    if (employee) setAvatar(avatar, employee, building);
    const row = h(doc, "div", "ow-tl");
    const what = h(doc, "span", "ow-tl__what");
    setRich(what, e.summary);
    row.append(
      h(doc, "span", "ow-tl__time", hms(e.ts)),
      avatar,
      h(doc, "span", "ow-tl__who", who(e)),
      what,
      e.confidence === "Inferred" ? h(doc, "span", "ow-est-tag", phrases.card.estimate) : "",
    );
    return row;
  }

  function fillTimeline(dto: SessionDetailDto, limit: number): void {
    if (!timeline) return;
    const legend = S.estimateLegend.startsWith(phrases.card.estimate)
      ? S.estimateLegend.slice(phrases.card.estimate.length)
      : S.estimateLegend;
    const top = h(
      doc,
      "div",
      "ow-pop__tlhead",
      h(doc, "h2", "ow-pop__h2", S.recent),
      h(doc, "span", "ow-mono ow-pop__n", String(dto.recent.length)),
      h(doc, "span", "ow-pop__legend", h(doc, "span", "ow-est-tag", phrases.card.estimate), legend),
    );
    const parts: Node[] = [top];
    if (dto.recentError) {
      const p = h(doc, "p", "ow-pop__error", detailErrorText(dto.recentError));
      p.setAttribute("role", "alert");
      parts.push(p);
    } else {
      parts.push(...dto.recent.map((e) => eventRow(e, dto.building)));
      // 20개를 채워 받았을 때만 더 있을 수 있다. 100개가 끝이다
      if (limit < MAX && dto.recent.length >= limit) {
        const more = h(doc, "button", "ow-pop__more", S.more);
        more.setAttribute("type", "button");
        more.append(h(doc, "span", "ow-mono", S.moreLimit));
        more.addEventListener("click", () => {
          more.disabled = true;
          void load(MAX);
        });
        parts.push(more);
      }
    }
    timeline.replaceChildren(...parts);
  }

  // 더 보기가 실패하면 받아 둔 목록은 그대로 두고 버튼을 되살려 다시 누를 수 있게 한다
  function showTimelineError(e: unknown): void {
    const more = timeline?.querySelector<HTMLButtonElement>(".ow-pop__more");
    if (more) more.disabled = false;
    timeline?.querySelector(".ow-pop__error")?.remove();
    const p = h(doc, "p", "ow-pop__error", detailErrorText(e));
    p.setAttribute("role", "alert");
    timeline?.append(p);
  }

  void load(FIRST);
}
