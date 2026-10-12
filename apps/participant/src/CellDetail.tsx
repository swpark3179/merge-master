import type { CellRef, CellValue, ColumnRule } from "@mm/shared";
import { a1, displayValue, editText, FLAG, normalize } from "@mm/shared";
import { useEffect, useRef, useState } from "react";

import { sameValue } from "./edits";
import type { SessionState } from "./useSession";

interface Props {
  s: SessionState;
  /** 지금 고른 칸(모델 좌표). */
  at: CellRef | null;
  columns: ColumnRule[];
  labels: string[];
  flags: Uint8Array;
  canEdit: (row: number, col: number) => boolean;
  /** 그 칸을 편집 중인 다른 사람. */
  locker?: string;
  /** '끝나면 알려 주기'를 걸어 두었다. */
  waiting: boolean;
  onWait: () => void;
}

function formatTag(rule: ColumnRule | undefined): string {
  switch (rule?.format.kind) {
    case "number":
      return rule.format.unit ? `숫자 · ${rule.format.unit}` : "숫자";
    case "list":
      return "목록";
    case "date":
      return "날짜";
    default:
      return "글자";
  }
}

/**
 * 사이드바 '셀 내용': 고른 칸의 위치·형식·안내와 내용 전체. 칸이 작아 다 보이지 않는 긴 글·여러 줄(Alt+Enter)을 여기서
 * 보고 고친다. 입력할 수 있는 칸이면 고친 뒤 칸을 벗어나거나 Ctrl+Enter로 저장, Esc로 취소한다. 고치기 시작하면 그 칸을
 * 잠가 다른 사람에게 '입력 중'으로 보인다.
 */
export function CellDetail({ s, at, columns, labels, flags, canEdit, locker, waiting, onWait }: Props) {
  const sheet = s.sheet!;
  const model = sheet.model;
  const rule = at ? columns[at.col] : undefined;
  const value: CellValue = at ? sheet.value(at.row, at.col) : null;
  const cell = at ? model.cells[at.row * model.cols + at.col] : undefined;
  const f = at ? (flags[at.row * model.cols + at.col] ?? 0) : 0;
  const editable = at !== null && canEdit(at.row, at.col) && !locker;
  const original = editText(value, rule?.format);

  const [draft, setDraft] = useState(original);
  const [dirty, setDirty] = useState(false);
  const [error, setError] = useState<string | null>(null);
  /** 이 패널에서 잠근 칸(고치기 시작할 때 잡고, 저장·취소하면 놓는다). */
  const locked = useRef<CellRef | null>(null);
  const key = at ? `${at.row}:${at.col}` : "";

  // 다른 칸을 고르면 새로 시작한다. 고치는 중이 아니면 남이 바꾼 값도 따라간다.
  useEffect(() => {
    setDirty(false);
    setError(null);
  }, [key]);
  useEffect(() => {
    if (!dirty) setDraft(original);
  }, [original, dirty, key]);

  const release = () => {
    if (locked.current) s.unlock(locked.current);
    locked.current = null;
  };

  // 다른 사람이 먼저 잡았으면(잠금 거절) 고친 것을 버린다.
  const { lockEvent } = s;
  useEffect(() => {
    const l = locked.current;
    if (
      lockEvent?.kind !== "denied" ||
      !l ||
      lockEvent.sheet !== s.active ||
      l.row !== lockEvent.at.row ||
      l.col !== lockEvent.at.col
    )
      return;
    locked.current = null;
    setDirty(false);
    setError(lockEvent.by ? `${lockEvent.by} 님이 입력 중입니다` : (lockEvent.reason ?? "입력할 수 없습니다"));
  }, [lockEvent]);

  // 칸을 바꾸거나 패널을 닫을 때 잡은 잠금을 놓는다.
  useEffect(() => () => release(), [key]); // eslint-disable-line react-hooks/exhaustive-deps

  if (!at || !cell) {
    return <div className="side-body muted small">칸을 고르면 그 칸의 내용이 여기에 보입니다.</div>;
  }

  const change = (text: string) => {
    if (!locked.current) {
      s.lock(at);
      locked.current = at;
    }
    setDraft(text);
    setDirty(true);
    setError(null);
  };

  /** `text`로 저장한다. 형식이 맞지 않으면 오류를 보이고 고친 것은 남긴다. */
  const commit = (text: string): boolean => {
    const n = normalize(rule?.format ?? { kind: "text" }, text);
    if (!n.ok) {
      setError(n.error);
      return false;
    }
    setDirty(false);
    setError(null);
    if (!sameValue(value, n.value) && s.edit([{ row: at.row, col: at.col, value: n.value }])) {
      locked.current = null; // 서버가 저장하면서 잠금을 푼다
    } else {
      release();
    }
    return true;
  };

  const save = () => (dirty ? commit(draft) : (release(), true));

  const cancel = () => {
    setDraft(original);
    setDirty(false);
    setError(null);
    release();
  };

  const where = a1(model.origin.row + at.row, model.origin.col + at.col);
  const lines = draft === "" ? 0 : draft.split("\n").length;
  const shown = displayValue(value, model.styles[cell.s]?.numberFormat ?? "", rule?.format);
  const items = rule?.format.kind === "list" ? rule.format.items : null;

  return (
    <div className="side-body cell-detail" aria-label="셀 내용">
      <div className="detail-head">
        <span className="addr mono">{where}</span>
        <b>{labels[at.col] || `열 ${at.col + 1}`}</b>
      </div>
      <div className="cell-tags">
        {f & FLAG.HEADER ? <span className="tag">머리글</span> : null}
        {f & FLAG.REQUIRED ? <span className="tag tag-required">필수</span> : null}
        {f & FLAG.INPUT ? <span className="tag">{formatTag(rule)}</span> : null}
        {f & FLAG.MINE ? <span className="tag tag-mine">내가 입력</span> : null}
        {cell.f ? (
          <span className="tag">수식</span>
        ) : !(f & FLAG.INPUT) && !(f & FLAG.HEADER) ? (
          <span className="tag">잠금</span>
        ) : null}
      </div>
      {locker && (
        <div className="hint-bar lock" role="status">
          <span className="icon-lock" aria-hidden />
          {locker} 님이 입력 중입니다 · 끝나면 입력할 수 있어요
          {waiting ? (
            <span className="muted"> · 끝나면 알려 드릴게요</span>
          ) : (
            <button className="link" onClick={onWait}>
              끝나면 알려 주기
            </button>
          )}
        </div>
      )}
      {f & FLAG.MINE && rule?.hint ? <div className="hint-bar">{rule.hint}</div> : null}
      {editable && items ? (
        <select
          aria-label="셀 내용 편집"
          className="detail-select"
          value={original}
          onChange={(e) => commit(e.target.value)}
        >
          <option value="">(비움)</option>
          {[...new Set([...(original && !items.includes(original) ? [original] : []), ...items])].map((i) => (
            <option key={i} value={i}>
              {i}
            </option>
          ))}
        </select>
      ) : editable ? (
        <textarea
          aria-label="셀 내용 편집"
          className="detail-text"
          value={draft}
          spellCheck={false}
          placeholder={rule?.format.kind === "date" ? "YYYY-MM-DD" : "내용 입력 · Enter로 줄바꿈"}
          onChange={(e) => change(e.target.value)}
          onBlur={() => save()}
          onKeyDown={(e) => {
            if (e.nativeEvent.isComposing) return;
            if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) {
              e.preventDefault();
              save();
            } else if (e.key === "Escape") {
              e.preventDefault();
              cancel();
            }
          }}
        />
      ) : (
        <div className="detail-value" aria-label="셀 내용">
          {shown === "" ? <span className="muted">(빈 칸)</span> : shown}
        </div>
      )}
      {cell.f && <div className="muted small mono">{cell.f}</div>}
      {error && <div className="cell-error-text">{error}</div>}
      <div className="detail-foot muted small">
        {editable ? (
          <>
            {lines > 1 ? `${lines}줄 · ` : ""}
            {[...draft].length.toLocaleString()}자 · 칸을 벗어나거나 Ctrl+Enter로 저장 · Esc 취소
            {dirty && <span className="detail-dirty"> · 저장 전</span>}
          </>
        ) : s.readOnly ? (
          "마감되어 읽기만 할 수 있습니다"
        ) : locker ? (
          "다른 사람이 입력을 끝내면 고칠 수 있습니다"
        ) : (
          "이 칸은 읽기만 할 수 있습니다"
        )}
      </div>
    </div>
  );
}
