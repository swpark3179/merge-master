import type { KeyboardEvent, RefObject } from "react";
import { useEffect, useState } from "react";

/** 편집을 끝내고 움직일 방향. `null`이면 그 자리. */
export type Move = readonly [-1 | 0 | 1, -1 | 0 | 1] | null;

export interface EditorState {
  text: string;
  /** 입력으로 시작했으면 방향키가 확정+이동, F2·더블클릭이면 글자 사이 이동(Excel의 입력/편집 모드). */
  arrowsMove: boolean;
  listOpen: boolean;
  error?: string;
}

interface Props {
  inputRef: RefObject<HTMLTextAreaElement | null>;
  /** `null`이면 대기(투명): 활성 칸에 미리 놓아 두어 첫 글자(한글 조합 포함)를 놓치지 않는다. */
  state: EditorState | null;
  /** 목록 형식의 항목. */
  items?: string[];
  placeholder?: string;
  onStart: (text: string) => void;
  onText: (text: string) => void;
  onCommit: (move: Move, text?: string) => void;
  /** 편집 중에 포커스가 편집기 밖으로 나갔다(다른 곳을 누름). */
  onLeave: () => void;
  onCancel: () => void;
  onToggleList: () => void;
}

/** 칸 안 편집기. 그리드 오버레이로 활성 칸 위에 놓인다. */
export function CellEditor({
  inputRef,
  state,
  items,
  placeholder,
  onStart,
  onText,
  onCommit,
  onLeave,
  onCancel,
  onToggleList,
}: Props) {
  const editing = state !== null;
  const text = state?.text ?? "";
  const filtered =
    items && state?.listOpen
      ? text === "" || items.some((i) => i === text)
        ? items
        : items.filter((i) => i.includes(text.trim()))
      : [];
  const [hi, setHi] = useState(-1);
  useEffect(() => {
    setHi(items && text ? items.indexOf(text) : -1);
  }, [items, text, state?.listOpen]);

  const onKeyDown = (e: KeyboardEvent<HTMLTextAreaElement>) => {
    // 대기 중에는 방향키·단축키를 그리드로 넘긴다(그리드가 이동·지우기·되돌리기를 한다).
    if (!editing) return;
    e.stopPropagation();
    if (e.nativeEvent.isComposing || e.keyCode === 229) return; // 한글 조합 중 Enter는 조합 확정
    const done = (move: Move, value?: string) => {
      e.preventDefault();
      onCommit(move, value);
    };
    if (state.listOpen && filtered.length > 0 && (e.key === "ArrowDown" || e.key === "ArrowUp")) {
      e.preventDefault();
      const step = e.key === "ArrowDown" ? 1 : -1;
      setHi((h) => (h + step + filtered.length) % filtered.length);
      return;
    }
    switch (e.key) {
      case "Enter":
        if (e.altKey) {
          // Alt+Enter: 칸 안 줄바꿈
          e.preventDefault();
          const el = e.currentTarget;
          const at = el.selectionStart;
          onText(text.slice(0, at) + "\n" + text.slice(el.selectionEnd));
          requestAnimationFrame(() => el.setSelectionRange(at + 1, at + 1));
          return;
        }
        return done(e.shiftKey ? [-1, 0] : [1, 0], state.listOpen && hi >= 0 ? filtered[hi] : undefined);
      case "Tab":
        return done(e.shiftKey ? [0, -1] : [0, 1]);
      case "Escape":
        e.preventDefault();
        if (state.listOpen) return onToggleList();
        return onCancel();
      case "ArrowDown":
        if (e.altKey && items) {
          e.preventDefault();
          return onToggleList();
        }
        if (state.arrowsMove) return done([1, 0]);
        return;
      case "ArrowUp":
        if (state.arrowsMove) return done([-1, 0]);
        return;
      case "ArrowLeft":
        if (state.arrowsMove) return done([0, -1]);
        return;
      case "ArrowRight":
        if (state.arrowsMove) return done([0, 1]);
        return;
    }
  };

  return (
    <div className={`cell-editor${editing ? " on" : ""}`}>
      <textarea
        ref={inputRef}
        aria-label="셀 입력"
        rows={1}
        spellCheck={false}
        value={text}
        placeholder={editing ? placeholder : undefined}
        onChange={(e) => (editing ? onText(e.target.value) : onStart(e.target.value))}
        onCompositionStart={() => {
          if (!editing) onStart("");
        }}
        onKeyDown={onKeyDown}
        onBlur={() => {
          if (editing) onLeave();
        }}
      />
      {items && !editing && (
        <button
          className="list-toggle"
          tabIndex={-1}
          aria-label="목록 열기"
          onMouseDown={(e) => {
            e.preventDefault();
            onToggleList();
          }}
        >
          ▾
        </button>
      )}
      {editing && state.listOpen && filtered.length > 0 && (
        <ul className="cell-list" role="listbox" aria-label="목록">
          {filtered.map((item, i) => (
            <li
              key={item}
              role="option"
              aria-selected={i === hi}
              className={i === hi ? "hi" : undefined}
              // 누르는 동안 편집기가 포커스를 잃어(blur) 먼저 확정되지 않게 한다.
              onMouseDown={(e) => {
                e.preventDefault();
                onCommit(null, item);
              }}
            >
              {item}
            </li>
          ))}
        </ul>
      )}
      {editing && state.error && (
        <div className="cell-error" role="alert">
          {state.error}
        </div>
      )}
    </div>
  );
}
