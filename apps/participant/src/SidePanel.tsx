import type { CellRange, CellRef, CellValue, ChatTo } from "@mm/shared";
import { a1, splitCellRefs } from "@mm/shared";
import { useEffect, useRef, useState } from "react";

import { sameValue } from "./edits";
import type { SessionState } from "./useSession";

export type PanelTab = "alerts" | "chat" | "history";

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
  tab: PanelTab;
  onTab: (t: PanelTab) => void;
  onClose: () => void;
  /** 지금 고른 칸(셀 이력). */
  at: CellRef | null;
  labels: string[];
  canEdit: (row: number, col: number) => boolean;
  /** 모델 좌표 범위로 간다. `highlight`가 있으면 확인으로 표시한다. */
  onReveal: (range: CellRange, highlight?: number) => void;
  onChatSeen: (id: number) => void;
}

/** 1f 오른쪽 패널: 알림(강조된 셀·공지), 채팅(전체·호스트에게), 셀 이력(내 변경 되돌리기). */
export function SidePanel({ s, tab, onTab, onClose, at, labels, canEdit, onReveal, onChatSeen }: Props) {
  const w = s.welcome!;
  const origin = s.sheet!.model.origin;
  const addr = (r: CellRef) => a1(origin.row + r.row, origin.col + r.col);
  const unreadAlerts = s.highlights.filter((h) => !h.acked).length + s.announcements.filter((n) => !n.read).length;

  return (
    <aside className="side-panel" aria-label="알림·채팅·셀 이력">
      <div className="side-tabs" role="tablist">
        <button role="tab" aria-selected={tab === "alerts"} onClick={() => onTab("alerts")}>
          알림{unreadAlerts > 0 ? ` ${unreadAlerts}` : ""}
        </button>
        <button role="tab" aria-selected={tab === "chat"} onClick={() => onTab("chat")}>
          채팅
        </button>
        <button role="tab" aria-selected={tab === "history"} onClick={() => onTab("history")}>
          셀 이력
        </button>
        <span className="spacer" />
        <button className="ghost small" aria-label="패널 닫기" onClick={onClose}>
          ✕
        </button>
      </div>
      {tab === "alerts" && <Alerts s={s} addr={addr} onReveal={onReveal} />}
      {tab === "chat" && (
        <Chat
          s={s}
          me={w.name}
          onRef={(r) => onReveal({ row: r.row - origin.row, col: r.col - origin.col, rows: r.rows, cols: r.cols })}
          onSeen={onChatSeen}
        />
      )}
      {tab === "history" && <CellHistory s={s} at={at} addr={addr} labels={labels} canEdit={canEdit} />}
    </aside>
  );
}

function Alerts({ s, addr, onReveal }: { s: SessionState; addr: (r: CellRef) => string; onReveal: Props["onReveal"] }) {
  const highlights = [...s.highlights].reverse();
  const notices = [...s.announcements].reverse();
  return (
    <div className="side-body">
      {highlights.length > 0 && <b className="small">강조된 셀</b>}
      {highlights.map((h) => (
        <div key={`h${h.id}`} className={`alert-item highlight${h.acked ? " done" : ""}`}>
          <div className="small">
            <span className="mono">{addr(h.range)}</span> · {h.from} · {time(h.atMs)}
            {h.acked && <span className="muted"> · 확인함</span>}
          </div>
          {h.note && <div>{h.note}</div>}
          <button className="link" onClick={() => onReveal(h.range, h.id)}>
            셀로 이동 →
          </button>
        </div>
      ))}
      {notices.length > 0 && <b className="small">공지</b>}
      {notices.map((n) => (
        <div key={`n${n.id}`} className={`alert-item${n.important ? " important" : ""}${n.read ? " done" : ""}`}>
          <div className="small">
            {n.important && <span className="tag tag-required">중요</span>} {time(n.atMs)} · {n.from}
            {n.read && <span className="muted"> · 확인함</span>}
          </div>
          <div>{n.text}</div>
          {!n.read && (
            <button className="ghost small" onClick={() => s.ackNotice(n.id)}>
              확인
            </button>
          )}
        </div>
      ))}
      {highlights.length === 0 && notices.length === 0 && <div className="muted small">받은 알림이 없습니다.</div>}
    </div>
  );
}

function Chat({
  s,
  me,
  onRef,
  onSeen,
}: {
  s: SessionState;
  me: string;
  onRef: (r: CellRange) => void;
  onSeen: (id: number) => void;
}) {
  const [toHost, setToHost] = useState(false);
  const [text, setText] = useState("");
  const list = useRef<HTMLDivElement>(null);
  const last = s.chat[s.chat.length - 1]?.id ?? 0;
  useEffect(() => {
    onSeen(last);
    list.current?.scrollTo({ top: list.current.scrollHeight });
  }, [last, onSeen]);
  const send = () => {
    const to: ChatTo = toHost ? { kind: "host" } : { kind: "all" };
    if (!text.trim()) return;
    s.sendChat(to, text);
    setText("");
  };
  return (
    <div className="side-body chat">
      <div className="chat-list" ref={list} aria-label="채팅 내용">
        {s.chat.length === 0 && (
          <div className="muted small">아직 채팅이 없습니다. 호스트에게 물어볼 것이 있으면 적어 주세요.</div>
        )}
        {s.chat.map((m) => (
          <div key={m.id} className={`chat-msg${m.from === me && !m.host ? " mine" : ""}`}>
            <div className="small">
              <b>{m.host ? (m.from === "호스트" ? "호스트" : `${m.from} · 호스트`) : m.from === me ? "나" : m.from}</b>
              {m.to.kind === "host" && <span className="muted"> → 호스트</span>}
              {m.to.kind === "person" && <span className="muted"> → 나에게</span>}
              <span className="muted"> · {time(m.atMs)}</span>
            </div>
            <div>
              {splitCellRefs(m.text).map((p, i) =>
                "ref" in p ? (
                  <button key={i} className="cell-ref" onClick={() => onRef(p.range)}>
                    #{p.ref}
                  </button>
                ) : (
                  <span key={i}>{p.text}</span>
                ),
              )}
            </div>
          </div>
        ))}
      </div>
      <label className="check small">
        <input type="checkbox" checked={toHost} onChange={(e) => setToHost(e.target.checked)} />
        호스트에게만
      </label>
      <div className="chat-input">
        <input
          aria-label="채팅 입력"
          placeholder="메시지 · #셀 참조(예: #F7)"
          maxLength={1000}
          value={text}
          onChange={(e) => setText(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && !e.nativeEvent.isComposing) send();
          }}
        />
        <button className="small" disabled={!text.trim()} onClick={send}>
          전송
        </button>
      </div>
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
  }, [row, sheet.myChanges, rowHistory]);
  if (!at) {
    return <div className="side-body muted small">칸을 고르면(오른쪽 클릭 → 셀 이력) 그 칸의 변경이 보입니다.</div>;
  }
  const entries = s.history?.row === at.row ? s.history.entries.filter((e) => e.col === at.col) : [];
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
