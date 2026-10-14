import type { CellPick, ColorOf, PreviewOf, TimelineItem } from "@mm/chat";
import { buildRefPreview, ChatBubble, ChatComposer, ChatText, ChatTimeline, ReactionChips } from "@mm/chat";
import type { CellRange, CellRef, CellValue, ChatTo, ColumnRule } from "@mm/shared";
import { a1, displayValue, personColor, sheetPrefix } from "@mm/shared";
import type { KeyboardEvent, PointerEvent, RefObject } from "react";
import { useEffect, useRef } from "react";

import type { CellLeave } from "./CellDetail";
import { CellDetail } from "./CellDetail";
import { sameValue } from "./edits";
import type { SessionState } from "./useSession";

export type PanelTab = "cell" | "alerts" | "chat" | "history";

/** 사이드바 기본 너비. 왼쪽 경계를 끌어 바꾼다(그리드 쪽은 적어도 360px 남긴다). */
export const SIDE_WIDTH = 310;
const SIDE_MIN = 260;
const SIDE_MAX = 720;

export function clampSideWidth(w: number): number {
  const max = Math.max(SIDE_MIN, Math.min(SIDE_MAX, window.innerWidth - 360));
  return Math.round(Math.min(max, Math.max(SIDE_MIN, w)));
}

function time(ms: number): string {
  const d = new Date(ms);
  return [d.getHours(), d.getMinutes()].map((n) => String(n).padStart(2, "0")).join(":");
}

function show(v: CellValue | undefined): string {
  if (v === undefined || v === null || v === "") return "(빈 값)";
  if (typeof v === "number") return v.toLocaleString(undefined, { maximumFractionDigits: 6 });
  if (typeof v === "boolean") return v ? "TRUE" : "FALSE";
  if (typeof v === "object") return `#오류(${v.err})`;
  return v;
}

const KIND: Record<string, string> = { input: "입력", edit: "수정", delete: "삭제", revert: "되돌림" };

interface Props {
  s: SessionState;
  /** 방금 사람이 열었다(미끄러져 들어온다). */
  slide?: boolean;
  tab: PanelTab;
  onTab: (t: PanelTab) => void;
  onClose: () => void;
  /** 너비(px). 끄는 동안은 화면에서만 바꾸고, 놓으면 `onWidth`로 알린다. */
  width: number;
  onWidth: (w: number) => void;
  /** 지금 고른 칸(셀 내용·셀 이력). */
  at: CellRef | null;
  labels: string[];
  columns: ColumnRule[];
  flags: Uint8Array;
  canEdit: (row: number, col: number) => boolean;
  /** 고른 칸을 편집 중인 다른 사람. */
  locker?: string;
  /** 고른 칸에 '끝나면 알려 주기'를 걸어 두었다. */
  waiting: boolean;
  onWait: () => void;
  /** '셀 내용'에서 고치던 것을 칸을 옮기기 전에 저장한다(Workspace가 부른다). */
  cellLeave: RefObject<CellLeave | null>;
  unreadChat: number;
  /** 그 시트(없으면 지금 시트)의 모델 좌표 범위로 간다. `highlight`가 있으면 확인으로 표시한다. */
  onReveal: (range: CellRange, highlight?: number, sheet?: number) => void;
  /** 채팅의 셀 참조(시트 주소, 시트 이름이 있으면 그 시트). */
  onRef: (range: CellRange, sheet?: string) => void;
  onChatSeen: (id: number) => void;
  /** 채팅 탭을 열기 전에 본 마지막 글(구분선 자리). */
  chatSeen: number;
  /** 채팅의 '@ 셀 선택'(그리드와 함께 쓴다). */
  pick: CellPick;
  /** 채팅 초안·'님에게만'(탭을 바꾸거나 닫아도 남도록 위에서 받는다). */
  chatDraft: string;
  onChatDraft: (text: string) => void;
  chatToHost: boolean;
  onChatToHost: (on: boolean) => void;
  /** 내가 숨긴 강조 메모(말풍선)와 그 토글. */
  hiddenNotes: Set<number>;
  onToggleNote: (id: number) => void;
}

/** 탭 이름 옆 개수(알약). 접근성 이름은 '알림 3'처럼 이어진다. */
function TabCount({ n }: { n: number }) {
  if (n <= 0) return null;
  return (
    <>
      {" "}
      <span key={n} className="tab-count">
        {n}
      </span>
    </>
  );
}

/** 1f 오른쪽 사이드바: 셀 내용(고른 칸의 위치·내용 보기·고치기), 알림(강조된 셀·공지), 채팅(전체·취합을 연 사람에게), 셀 이력. */
export function SidePanel(props: Props) {
  const { s, tab, onTab, onClose, at, labels, canEdit, onReveal, onRef, onChatSeen } = props;
  const w = s.welcome!;
  const origin = s.sheet!.model.origin;
  const addr = (r: CellRef) => a1(origin.row + r.row, origin.col + r.col);
  /** 다른 시트의 주소(시트가 여럿이면 시트 이름을 붙인다). */
  const sheetAddr = (sheet: number, r: CellRef) => {
    const m = s.sheets[sheet]?.model;
    if (!m) return "";
    return `${s.sheets.length > 1 ? sheetPrefix(m.sheet) : ""}${a1(m.origin.row + r.row, m.origin.col + r.col)}`;
  };
  const unreadAlerts = s.highlights.filter((h) => !h.acked).length + s.announcements.filter((n) => !n.read).length;
  const panel = useRef<HTMLElement>(null);
  const { width, onWidth } = props;

  // 왼쪽 경계 끌기: 끄는 동안은 DOM 너비만 바꿔 작업 화면을 다시 그리지 않는다(그리드는 크기 변화를 스스로 잰다).
  const startResize = (e: PointerEvent<HTMLDivElement>) => {
    const el = panel.current;
    if (e.button !== 0 || !el) return;
    e.preventDefault();
    const handle = e.currentTarget;
    const x0 = e.clientX;
    const w0 = el.getBoundingClientRect().width;
    let w = w0;
    const move = (ev: globalThis.PointerEvent) => {
      w = clampSideWidth(w0 + x0 - ev.clientX);
      el.style.width = `${w}px`;
    };
    const end = () => {
      handle.removeEventListener("pointermove", move);
      handle.removeEventListener("pointerup", end);
      handle.removeEventListener("pointercancel", end);
      handle.classList.remove("dragging");
      document.body.classList.remove("side-resizing");
      onWidth(w);
    };
    handle.setPointerCapture(e.pointerId);
    handle.addEventListener("pointermove", move);
    handle.addEventListener("pointerup", end);
    handle.addEventListener("pointercancel", end);
    handle.classList.add("dragging");
    document.body.classList.add("side-resizing");
  };
  const resizeKey = (e: KeyboardEvent<HTMLDivElement>) => {
    const step = e.shiftKey ? 64 : 16;
    if (e.key === "ArrowLeft") onWidth(clampSideWidth(width + step));
    else if (e.key === "ArrowRight") onWidth(clampSideWidth(width - step));
    else if (e.key === "Home") onWidth(clampSideWidth(SIDE_WIDTH));
    else return;
    e.preventDefault();
  };

  return (
    <aside
      className={`side-panel${props.slide ? " slide-in" : ""}`}
      aria-label="사이드바"
      ref={panel}
      style={{ width }}
    >
      <div
        className="side-resize"
        role="separator"
        aria-orientation="vertical"
        aria-label="사이드바 너비"
        aria-valuenow={width}
        aria-valuemin={SIDE_MIN}
        aria-valuemax={SIDE_MAX}
        tabIndex={0}
        title="끌어서 사이드바 너비 조절 · 두 번 누르면 기본 너비"
        onPointerDown={startResize}
        onDoubleClick={() => onWidth(clampSideWidth(SIDE_WIDTH))}
        onKeyDown={resizeKey}
      />
      <div className="side-head">
        <div className="side-tabs" role="tablist" aria-label="사이드바 탭">
          <button role="tab" aria-selected={tab === "cell"} onClick={() => onTab("cell")}>
            셀 내용
          </button>
          <button role="tab" aria-selected={tab === "alerts"} onClick={() => onTab("alerts")}>
            알림
            <TabCount n={unreadAlerts} />
          </button>
          <button role="tab" aria-selected={tab === "chat"} onClick={() => onTab("chat")}>
            채팅
            <TabCount n={tab === "chat" ? 0 : props.unreadChat} />
          </button>
          <button role="tab" aria-selected={tab === "history"} onClick={() => onTab("history")}>
            셀 이력
          </button>
        </div>
        <button className="ghost side-close" aria-label="패널 닫기" title="사이드바 닫기" onClick={onClose}>
          ✕
        </button>
      </div>
      {tab === "cell" && (
        <CellDetail
          s={s}
          at={at}
          columns={props.columns}
          labels={labels}
          flags={props.flags}
          canEdit={canEdit}
          locker={props.locker}
          waiting={props.waiting}
          onWait={props.onWait}
          leaveRef={props.cellLeave}
        />
      )}
      {tab === "alerts" && (
        <Alerts
          s={s}
          addr={sheetAddr}
          onReveal={onReveal}
          hiddenNotes={props.hiddenNotes}
          onToggleNote={props.onToggleNote}
        />
      )}
      {tab === "chat" && (
        <Chat
          s={s}
          me={w.name}
          host={s.presence.find((x) => x.host)?.name || w.session.requester || "요청자"}
          onRef={onRef}
          seen={props.chatSeen}
          onSeen={onChatSeen}
          pick={props.pick}
          text={props.chatDraft}
          setText={props.onChatDraft}
          toHost={props.chatToHost}
          setToHost={props.onChatToHost}
        />
      )}
      {tab === "history" && <CellHistory s={s} at={at} addr={addr} labels={labels} canEdit={canEdit} />}
    </aside>
  );
}

/** 확인하지 않은 것을 먼저, 그 안에서는 최근 것부터. */
function unreadFirst<T extends { id: number }>(list: T[], done: (x: T) => boolean): T[] {
  return [...list].sort((a, b) => Number(done(a)) - Number(done(b)) || b.id - a.id);
}

/** 알림 탭: 강조된 셀과 공지를 카드로(확인하지 않은 것 먼저). 강조 메모는 말풍선을 숨기거나 다시 보인다. */
function Alerts({
  s,
  addr,
  onReveal,
  hiddenNotes,
  onToggleNote,
}: {
  s: SessionState;
  addr: (sheet: number, r: CellRef) => string;
  onReveal: Props["onReveal"];
  hiddenNotes: Set<number>;
  onToggleNote: (id: number) => void;
}) {
  const highlights = unreadFirst(s.highlights, (h) => h.acked);
  const notices = unreadFirst(s.announcements, (n) => n.read);
  const unreadNotices = notices.filter((n) => !n.read);
  return (
    <div className="side-body alerts">
      {highlights.length > 0 && (
        <div className="side-section-head">
          <b>강조된 셀</b>
          <span className="count">{highlights.length}</span>
        </div>
      )}
      {highlights.map((h) => (
        <div key={`h${h.id}`} className={`alert-item highlight${h.acked ? " done" : ""}`}>
          <div className="alert-meta">
            <span className="mono addr">{addr(h.sheet, h.range)}</span>
            <span>
              {h.from} · {time(h.atMs)}
            </span>
            {h.acked ? <span className="state">확인함</span> : <span className="state new">새 강조</span>}
          </div>
          {h.note && <div className="alert-text">{h.note}</div>}
          <div className="alert-actions">
            <button className="link" onClick={() => onReveal(h.range, h.id, h.sheet)}>
              셀로 이동 →
            </button>
            {h.note && (
              <button
                className="link subtle"
                title="그리드 위 말풍선만 숨깁니다(이 브라우저에만, 노란 강조는 남습니다)"
                onClick={() => onToggleNote(h.id)}
              >
                {hiddenNotes.has(h.id) ? "메모 다시 보기" : "메모 숨기기"}
              </button>
            )}
          </div>
        </div>
      ))}
      {notices.length > 0 && (
        <div className="side-section-head">
          <b>공지</b>
          <span className="count">{notices.length}</span>
          <span className="spacer" />
          {unreadNotices.length > 1 && (
            <button className="link" onClick={() => unreadNotices.forEach((n) => s.ackNotice(n.id))}>
              모두 확인
            </button>
          )}
        </div>
      )}
      {notices.map((n) => (
        <div key={`n${n.id}`} className={`alert-item notice${n.important ? " important" : ""}${n.read ? " done" : ""}`}>
          <div className="alert-meta">
            {n.important && <span className="tag tag-required">중요</span>}
            <span>
              {n.from} · {time(n.atMs)}
            </span>
            {n.read && <span className="state">확인함</span>}
          </div>
          <div className="alert-text">{n.text}</div>
          {!n.read && (
            <div className="alert-actions">
              <button className="ghost small" onClick={() => s.ackNotice(n.id)}>
                확인
              </button>
            </div>
          )}
        </div>
      ))}
      {highlights.length === 0 && notices.length === 0 && (
        <div className="side-empty muted small">
          받은 알림이 없습니다. 호스트가 공지하거나 셀을 강조하면 여기에 모입니다.
        </div>
      )}
    </div>
  );
}

/** 채팅 한 통의 최대 글자 수(서버 `MAX_CHAT_CHARS`). */
const CHAT_MAX = 1000;

function Chat({
  s,
  me,
  host,
  onRef,
  seen,
  onSeen,
  pick,
  text,
  setText,
  toHost,
  setToHost,
}: {
  s: SessionState;
  me: string;
  /** 취합을 연 사람의 이름(1c '내 이름'). */
  host: string;
  onRef: (r: CellRange, sheet?: string) => void;
  seen: number;
  onSeen: (id: number) => void;
  pick: CellPick;
  text: string;
  setText: (text: string) => void;
  toHost: boolean;
  setToHost: (on: boolean) => void;
}) {
  const w = s.welcome!;
  const send = () => {
    const to: ChatTo = toHost ? { kind: "host" } : { kind: "all" };
    if (!text.trim()) return;
    s.sendChat(to, text);
    setText("");
  };
  const viewer = { name: me };
  const colorOf: ColorOf = (name, isHost) => (isHost ? "var(--ink)" : personColor(Math.max(0, w.roster.indexOf(name))));
  const preview: PreviewOf = (range, name) => {
    const i = name === undefined ? s.active : s.sheets.findIndex((x) => x.model.sheet === name);
    const m = s.sheets[i]?.model;
    if (!m) return null;
    const { columns, labels } = w.sheets[i] ?? { columns: [], labels: [] };
    const others = new Set(m.hiddenRows);
    return buildRefPreview(
      {
        sheet: m.sheet,
        origin: m.origin,
        rows: m.rows,
        cols: m.cols,
        text: (r, c) => {
          const cell = m.cells[r * m.cols + c];
          const nf = m.styles[cell?.s ?? 0]?.numberFormat ?? "";
          return { text: displayValue(cell?.v, nf, columns[c]?.format), numeric: typeof cell?.v === "number" };
        },
        label: (c) => labels[c],
        hidden: (r) => others.has(r),
      },
      { ...range, row: range.row - m.origin.row, col: range.col - m.origin.col },
    );
  };
  const items: TimelineItem[] = s.chat.map((m) => {
    const mine = m.from === me && !m.host;
    return {
      id: m.id,
      mine,
      node: (
        <ChatBubble
          mine={mine}
          name={m.from}
          color={colorOf(m.from, m.host)}
          head={
            <>
              <b style={mine ? undefined : { color: colorOf(m.from, m.host) }}>{mine ? "나" : m.from}</b>
              {m.to.kind === "host" && <span className="muted"> → {host}</span>}
              {m.to.kind === "person" && <span className="muted"> → 나에게</span>}
              <span className="muted"> · {time(m.atMs)}</span>
            </>
          }
          reactions={
            <ReactionChips
              marks={m.reactions}
              viewer={viewer}
              canReact={!mine}
              onToggle={(kind) => s.reactChat(m.id, kind)}
              colorOf={colorOf}
            />
          }
        >
          <ChatText text={m.text} onRef={onRef} preview={preview} />
        </ChatBubble>
      ),
    };
  });
  return (
    <div className="side-body chat">
      <ChatTimeline
        items={items}
        seen={seen}
        onSeen={onSeen}
        empty={<div className="muted small">아직 채팅이 없습니다. {host} 님에게 물어볼 것이 있으면 적어 주세요.</div>}
      />
      <ChatComposer
        value={text}
        onChange={setText}
        onSend={send}
        maxLength={CHAT_MAX}
        placeholder="메시지 · @로 셀 선택"
        pick={pick}
        options={
          <>
            <label className="check small">
              <input type="checkbox" checked={toHost} onChange={(e) => setToHost(e.target.checked)} />
              {host} 님에게만
            </label>
            <span className="muted small">@ 셀 선택 · Shift+Enter 줄바꿈</span>
          </>
        }
      />
    </div>
  );
}

function CellHistory({
  s,
  at,
  addr,
  labels,
  canEdit,
}: {
  s: SessionState;
  at: CellRef | null;
  addr: (r: CellRef) => string;
  labels: string[];
  canEdit: (row: number, col: number) => boolean;
}) {
  const sheet = s.sheet!;
  const me = s.welcome!.name;
  const { rowHistory } = s;
  const row = at?.row;
  useEffect(() => {
    if (row !== undefined) rowHistory(row);
  }, [row, sheet.myChanges, rowHistory, s.active]);
  if (!at) {
    return <div className="side-body muted small">칸을 고르면(오른쪽 클릭 → 셀 이력) 그 칸의 변경이 보입니다.</div>;
  }
  const entries =
    s.history?.sheet === s.active && s.history.row === at.row ? s.history.entries.filter((e) => e.col === at.col) : [];
  const current = sheet.value(at.row, at.col);
  const lastSeq = entries[entries.length - 1]?.seq;
  return (
    <div className="side-body">
      <b className="small">
        <span className="mono">{addr(at)}</span> {labels[at.col] ?? ""}
      </b>
      {entries.length === 0 && <div className="muted small">이 칸은 아직 바뀐 적이 없습니다.</div>}
      <ol className="cell-history">
        {[...entries].reverse().map((e) => {
          const mine = e.user === me;
          const revertable = mine && e.seq === lastSeq && sameValue(current, e.new) && canEdit(at.row, at.col);
          return (
            <li key={e.seq}>
              <span className="muted small">{time(e.atMs)}</span> {e.user === me ? "나" : e.user} {KIND[e.kind]}{" "}
              <span className="muted">{show(e.old)}</span> → <b>{show(e.new)}</b>
              {revertable && (
                <button
                  className="ghost small"
                  onClick={() => s.edit([{ row: at.row, col: at.col, value: e.old ?? null }], "revert")}
                >
                  되돌리기
                </button>
              )}
            </li>
          );
        })}
      </ol>
      <div className="muted small">
        내가 바꾼 마지막 값만 되돌릴 수 있습니다. 다른 사람의 변경은 채팅으로 호스트에게 요청해 주세요.
      </div>
    </div>
  );
}
