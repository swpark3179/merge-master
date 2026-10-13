import type { KeyboardEvent } from "react";
import { useEffect, useId, useMemo, useRef, useState } from "react";

import { matchOptions } from "./filters";

/**
 * 조회조건 콤보: 지금 고른 값을 보이고, 펼치면 검색 칸과 목록이 나온다(항목이 많아도 찾기 쉽게). 맨 위 '전체'는 조건을
 * 푼다. ↑↓로 고르고 Enter, Esc·바깥 클릭이면 닫힌다.
 */
export function SearchSelect({
  label,
  value,
  options,
  onChange,
}: {
  /** 열 이름(`부서`). 단추 이름은 `부서 조회조건`. */
  label: string;
  /** 고른 값, 빈 글자면 전체. */
  value: string;
  options: readonly string[];
  onChange: (value: string) => void;
}) {
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState("");
  const [hi, setHi] = useState(0);
  const root = useRef<HTMLDivElement>(null);
  const listRef = useRef<HTMLUListElement>(null);
  const id = useId();
  const shown = useMemo(() => {
    const hits = matchOptions(options, query);
    return query.trim() ? hits : ["", ...hits];
  }, [options, query]);

  useEffect(() => {
    if (!open) return;
    const outside = (e: MouseEvent) => {
      if (!root.current?.contains(e.target as Node)) setOpen(false);
    };
    document.addEventListener("mousedown", outside, true);
    return () => document.removeEventListener("mousedown", outside, true);
  }, [open]);

  // 강조한 항목이 목록 안에 보이게.
  useEffect(() => {
    listRef.current?.querySelector<HTMLElement>(`[data-i="${hi}"]`)?.scrollIntoView({ block: "nearest" });
  }, [hi, open]);

  const show = () => {
    setQuery("");
    setHi(Math.max(0, ["", ...options].indexOf(value)));
    setOpen(true);
  };

  const pick = (v: string) => {
    onChange(v);
    setOpen(false);
  };

  const onKey = (e: KeyboardEvent) => {
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      const n = shown.length;
      if (n > 0) setHi((i) => (e.key === "ArrowDown" ? (i + 1) % n : (i - 1 + n) % n));
    } else if (e.key === "Enter") {
      e.preventDefault();
      const v = shown[hi];
      if (v !== undefined) pick(v);
    } else if (e.key === "Escape") {
      e.preventDefault();
      setOpen(false);
    }
  };

  return (
    <div className="search-select" ref={root}>
      <button
        type="button"
        role="combobox"
        aria-label={`${label} 조회조건`}
        aria-haspopup="listbox"
        aria-expanded={open}
        aria-controls={open ? id : undefined}
        className={value ? "on" : undefined}
        title={value || "전체"}
        onClick={() => (open ? setOpen(false) : show())}
        onKeyDown={(e) => {
          if (!open && (e.key === "ArrowDown" || e.key === "Enter")) {
            e.preventDefault();
            show();
          }
        }}
      >
        <span className="value">{value || "전체"}</span>
        <span aria-hidden>▾</span>
      </button>
      {open && (
        <div className="search-select-pop">
          <input
            autoFocus
            type="search"
            aria-label={`${label} 검색`}
            placeholder="검색"
            value={query}
            onChange={(e) => {
              setQuery(e.target.value);
              setHi(0);
            }}
            onKeyDown={onKey}
          />
          <ul id={id} role="listbox" aria-label={`${label} 항목`} ref={listRef}>
            {shown.map((o, i) => (
              <li
                key={o === "" ? "\u0000all" : o}
                role="option"
                data-i={i}
                aria-selected={o === value}
                className={`${i === hi ? "hi" : ""}${o === "" ? " all" : ""}`}
                onMouseEnter={() => setHi(i)}
                // 누르는 동안 검색 칸이 포커스를 잃어 닫히지 않게.
                onMouseDown={(e) => {
                  e.preventDefault();
                  pick(o);
                }}
              >
                {o === "" ? "전체" : o}
              </li>
            ))}
            {shown.length === 0 && <li className="empty">일치하는 항목이 없습니다</li>}
          </ul>
        </div>
      )}
    </div>
  );
}
