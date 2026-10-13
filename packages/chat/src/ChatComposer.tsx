import { chatEnterKey } from "@mm/shared";
import type { ChangeEvent, KeyboardEvent, ReactNode } from "react";
import { useEffect, useLayoutEffect, useRef, useState } from "react";

import { GridIcon } from "./CellRefChip";
import { deleteToken, insertToken, mirrorSegments, pickInput, pickTrigger } from "./composer";
import type { CellPick } from "./useCellPick";

const ARROWS: Record<string, [-1 | 0 | 1, -1 | 0 | 1]> = {
  ArrowUp: [-1, 0],
  ArrowDown: [1, 0],
  ArrowLeft: [0, -1],
  ArrowRight: [0, 1],
};

/**
 * 채팅 입력 칸. 글에 맞춰 높아지고(최대 높이를 넘으면 스크롤), 셀 참조는 칩 바탕으로 보인다(뒤의 거울이 그린다:
 * textarea라 한글 조합·붙여넣기·Enter 규칙이 그대로다). Enter 보내기, Shift·Ctrl·Alt+Enter 줄바꿈.
 *
 * `pick`을 주면 '@ 셀 선택': 글 맨 앞이나 띄어쓰기 뒤에 `@`를 치면 선택 모드가 되어 그리드를 끌거나 방향키(Shift로
 * 넓히기)로 범위를 고르고, 마우스를 놓거나 Enter를 누르면 `#시트!범위` 참조가 들어간다. `@F6:H8 `처럼 주소를 쳐도 된다.
 * Esc·고르기 전 띄어쓰기·주소가 아닌 글자·입력 칸 밖을 누르면 선택 모드만 끝나고 `@`는 글자로 남는다.
 */
export function ChatComposer({
  value,
  onChange,
  onSend,
  maxLength,
  placeholder,
  pick,
  options,
}: {
  value: string;
  onChange: (text: string) => void;
  /** 보낼 글이 있을 때 Enter·전송 단추. 보낸 뒤 비우는 것은 쓰는 쪽이 한다. */
  onSend: () => void;
  maxLength: number;
  placeholder: string;
  pick?: CellPick;
  /** 입력 칸 위 한 줄(받는 사람·안내). */
  options?: ReactNode;
}) {
  const input = useRef<HTMLTextAreaElement>(null);
  /** 선택 모드를 연 `@`의 자리. */
  const [at, setAt] = useState<number | null>(null);
  /** 고르기로 넣은 참조(Backspace·Delete 한 번에 통째로 지운다). */
  const [picked, setPicked] = useState<string[]>([]);
  /** 다음에 그릴 때 둘 커서 자리. */
  const caret = useRef<number | null>(null);
  const picking = at !== null && pick !== undefined;

  useLayoutEffect(() => {
    const el = input.current;
    if (!el || caret.current === null) return;
    el.focus();
    el.setSelectionRange(caret.current, caret.current);
    caret.current = null;
  }, [value]);

  const exit = () => {
    setAt(null);
    pick?.end();
  };

  const put = (token: string, extra = " ") => {
    if (at === null) return;
    const done = insertToken(value, at, token, maxLength, extra);
    exit();
    if (!done) return;
    caret.current = done.caret;
    setPicked((p) => (p.includes(token) ? p : [...p, token]));
    onChange(done.text);
  };

  // 그리드에서 마우스를 놓으면 고른 범위를 넣는다.
  const commitFromGrid = useRef<() => void>(() => {});
  commitFromGrid.current = () => {
    if (picking && pick.current) put(pick.current.token);
  };
  useEffect(() => {
    if (!pick) return;
    pick.bindCommit(() => commitFromGrid.current());
    return () => pick.bindCommit(null);
  }, [pick]);

  // 보내서 비었다: 고르기 기록도 비운다.
  useEffect(() => {
    if (value !== "") return;
    setPicked([]);
    if (at !== null) exit();
    // 글이 비었을 때만.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [value]);

  const send = () => {
    if (!value.trim()) return;
    exit();
    onSend();
  };

  const change = (e: ChangeEvent<HTMLTextAreaElement>) => {
    const next = e.target.value;
    const pos = e.target.selectionStart;
    if (picking) {
      const r = pickInput(next, at, pos);
      if (r.kind === "commit") {
        // 주소 뒤에 친 띄어쓰기는 글에 그대로 있다: 참조로 바꾸고 커서는 그 띄어쓰기 뒤로.
        const token = pick.tokenOf(r.range);
        const done = insertToken(next, at, token, maxLength, "");
        exit();
        if (done) {
          caret.current = done.caret + 1;
          setPicked((p) => (p.includes(token) ? p : [...p, token]));
          onChange(done.text);
          return;
        }
      } else if (r.kind === "exit") exit();
      else pick.typed(r.range);
    } else if (pick && !(e.nativeEvent as InputEvent).isComposing) {
      const start = pickTrigger(value, next, pos);
      if (start !== null) {
        setAt(start);
        pick.begin();
      }
    }
    onChange(next);
  };

  const keyDown = (e: KeyboardEvent<HTMLTextAreaElement>) => {
    if (e.nativeEvent.isComposing || e.keyCode === 229) return;
    const mod = e.ctrlKey || e.metaKey || e.altKey;
    if (picking) {
      if (e.key === "Escape") {
        e.preventDefault();
        e.stopPropagation();
        exit();
        return;
      }
      const d = ARROWS[e.key];
      if (d && !mod) {
        e.preventDefault();
        pick.move(d[0], d[1], e.shiftKey);
        return;
      }
      if (e.key === "Enter" && !e.shiftKey && !mod) {
        e.preventDefault();
        if (pick.current) put(pick.current.token);
        else send();
        return;
      }
      // 끌기·방향키로 고른 뒤 글자를 치면 참조를 넣고 그 글자를 잇는다.
      if (pick.current && pick.source === "grid" && e.key.length === 1 && !mod) {
        e.preventDefault();
        put(pick.current.token, e.key === " " ? " " : /[\p{L}\p{N}]/u.test(e.key) ? ` ${e.key}` : e.key);
        return;
      }
      return;
    }
    const el = e.currentTarget;
    if ((e.key === "Backspace" || e.key === "Delete") && el.selectionStart === el.selectionEnd) {
      const done = deleteToken(value, el.selectionStart, e.key, picked);
      if (done) {
        e.preventDefault();
        caret.current = done.caret;
        onChange(done.text);
        return;
      }
    }
    const key = chatEnterKey(e);
    if (!key) return;
    e.preventDefault();
    if (key === "send") return send();
    const next = value.slice(0, el.selectionStart) + "\n" + value.slice(el.selectionEnd);
    if (next.length > maxLength) return;
    caret.current = el.selectionStart + 1;
    onChange(next);
  };

  // 거울: 참조는 칩 바탕, 선택 모드의 @는 표시, @가 끝에 있으면 고르는 중인 주소를 흐리게 뒤에 붙인다.
  const segments = mirrorSegments(value, picking ? at : null);
  const ghost = picking && pick.current && pick.source === "grid" && at === value.length - 1;

  return (
    <div className="chat-composer">
      {picking && (
        <div className="chat-pick-banner" onMouseDown={(e) => e.preventDefault()} role="status">
          <div className="row">
            <svg
              width="14"
              height="14"
              viewBox="0 0 16 16"
              fill="none"
              stroke="currentColor"
              strokeWidth="1.5"
              strokeLinecap="round"
              aria-hidden="true"
            >
              <path d="M8 1.5v4M8 10.5v4M1.5 8h4M10.5 8h4" />
              <circle cx="8" cy="8" r="1.2" />
            </svg>
            <b>셀 선택 중</b>
            {pick.current ? (
              <>
                <span className="chat-ref-static">
                  <GridIcon />
                  <span className="sheet">{pick.current.sheet}</span>
                  {pick.current.addr}
                </span>
                <span className="count">{pick.current.count.toLocaleString()}칸</span>
              </>
            ) : (
              <span className="count">그리드에서 범위를 끌어 고르세요</span>
            )}
          </div>
          <div className="keys">
            <span>
              <kbd>↑↓←→</kbd> 이동
            </span>
            <span>
              <kbd>Shift</kbd> 넓히기
            </span>
            <span>
              <kbd>Enter</kbd> 넣기
            </span>
            <span>
              <kbd>Esc</kbd> 글자 @로
            </span>
          </div>
        </div>
      )}
      {options && <div className="chat-options">{options}</div>}
      <div className="chat-compose-row">
        <div className={`chat-field${picking ? " picking" : ""}`}>
          <div className="chat-mirror" aria-hidden="true">
            {segments.map((s, i) => (
              <span key={i} className={s.kind === "text" ? undefined : s.kind}>
                {s.text}
              </span>
            ))}
            {ghost && <span className="ghost">{`${pick.current!.sheet}${pick.current!.addr}`}</span>}
            {/* 마지막 줄바꿈도 한 줄 높이를 갖게 */}{" "}
          </div>
          <textarea
            ref={input}
            aria-label="채팅 입력"
            placeholder={placeholder}
            rows={1}
            maxLength={maxLength}
            value={value}
            onChange={change}
            onKeyDown={keyDown}
            onBlur={() => picking && exit()}
          />
        </div>
        <button type="button" className="chat-send" disabled={!value.trim()} onClick={send}>
          전송
        </button>
      </div>
    </div>
  );
}

/** 고르는 중 그리드 위 안내(그리드를 감싼 요소가 `position: relative`여야 한다). */
export function PickHint() {
  return (
    <div className="chat-pick-hint-wrap" aria-hidden="true">
      <div className="chat-pick-hint">채팅에 넣을 셀이나 범위를 고르세요 · Esc 취소</div>
    </div>
  );
}
