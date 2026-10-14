import type { KeyboardEvent } from "react";
import { useEffect, useId, useMemo, useRef, useState } from "react";

import { matchOptions, selectionLabel, toggleHits, toggleValue } from "./filters";

/** 펼친 목록의 한 줄: '전체'(조건 풀기), '검색 결과 모두 선택', 항목. */
type Row = { kind: "all" } | { kind: "hits" } | { kind: "item"; value: string };

/**
 * 조회조건 콤보(여러 개 고르기): 지금 고른 값을 보이고, 펼치면 검색 칸과 체크 목록이 나온다(항목이 많아도 찾기 쉽게).
 * 항목을 누를 때마다 고르거나 빼고 목록은 열린 채로 있다. 맨 위는 '전체'(조건을 푼다), 검색어가 있으면 '검색 결과 모두 선택'.
 * ↑↓로 옮기고 Enter로 고르거나 빼며, Esc·바깥 클릭이면 닫힌다.
 */
export function SearchSelect({
  label,
  value,
  options,
  onChange,
}: {
  /** 열 이름(`부서`). 단추 이름은 `부서 조회조건`. */
  label: string;
  /** 고른 값들(고른 순서), 비면 전체. */
  value: readonly string[];
  options: readonly string[];
  onChange: (value: string[]) => void;
}) {
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState("");
  const [hi, setHi] = useState(0);
  const root = useRef<HTMLDivElement>(null);
  const listRef = useRef<HTMLUListElement>(null);
  const id = useId();
  // 고른 값이 그새 목록에서 사라졌어도(값이 바뀜) 보여서 뺄 수 있게 한다.
  const items = useMemo(() => [...options, ...value.filter((v) => !options.includes(v))], [options, value]);
  const hits = useMemo(() => matchOptions(items, query), [items, query]);
  const searching = query.trim() !== "";
  const rows = useMemo<Row[]>(() => {
    const list = hits.map((v): Row => ({ kind: "item", value: v }));
    if (!searching) return [{ kind: "all" }, ...list];
    return hits.length > 0 ? [{ kind: "hits" }, ...list] : [];
  }, [hits, searching]);
  const allHits = hits.length > 0 && hits.every((h) => value.includes(h));

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
    setHi(0);
    setOpen(true);
  };

  const choose = (row: Row) => {
    if (row.kind === "all") onChange([]);
    else if (row.kind === "hits") onChange(toggleHits(value, hits));
    else onChange(toggleValue(value, row.value));
  };

  const onKey = (e: KeyboardEvent) => {
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      const n = rows.length;
      if (n > 0) setHi((i) => (e.key === "ArrowDown" ? (i + 1) % n : (i - 1 + n) % n));
    } else if (e.key === "Enter") {
      e.preventDefault();
      const row = rows[hi];
      if (row) choose(row);
    } else if (e.key === "Escape") {
      e.preventDefault();
      setOpen(false);
    }
  };

  const shown = selectionLabel(value);
  return (
    <div className="search-select" ref={root}>
      <button
        type="button"
        role="combobox"
        aria-label={`${label} 조회조건`}
        aria-haspopup="listbox"
        aria-expanded={open}
        aria-controls={open ? id : undefined}
        className={value.length > 0 ? "on" : undefined}
        title={value.length > 0 ? value.join(", ") : "전체"}
        onClick={() => (open ? setOpen(false) : show())}
        onKeyDown={(e) => {
          if (!open && (e.key === "ArrowDown" || e.key === "Enter")) {
            e.preventDefault();
            show();
          }
        }}
      >
        <span className="value">{shown}</span>
        <span aria-hidden>▾</span>
      </button>
      {open && (
        <div className="search-select-pop">
          <input
            autoFocus
            type="search"
            aria-label={`${label} 검색`}
            placeholder="검색 · 여러 개 고르기"
            value={query}
            onChange={(e) => {
              setQuery(e.target.value);
              setHi(0);
            }}
            onKeyDown={onKey}
          />
          <ul id={id} role="listbox" aria-multiselectable="true" aria-label={`${label} 항목`} ref={listRef}>
            {rows.map((row, i) => {
              const on =
                row.kind === "all" ? value.length === 0 : row.kind === "hits" ? allHits : value.includes(row.value);
              const some = row.kind === "hits" && !allHits && hits.some((h) => value.includes(h));
              return (
                <li
                  key={row.kind === "item" ? `v:${row.value}` : row.kind}
                  role="option"
                  data-i={i}
                  aria-selected={on}
                  className={`${i === hi ? "hi" : ""}${row.kind !== "item" ? " all" : ""}${some ? " some" : ""}`}
                  onMouseEnter={() => setHi(i)}
                  // 누르는 동안 검색 칸이 포커스를 잃어 닫히지 않게.
                  onMouseDown={(e) => {
                    e.preventDefault();
                    choose(row);
                  }}
                >
                  <span className="box" aria-hidden />
                  <span className="text">
                    {row.kind === "all"
                      ? "전체"
                      : row.kind === "hits"
                        ? `검색 결과 모두 선택 (${hits.length})`
                        : row.value}
                  </span>
                </li>
              );
            })}
            {rows.length === 0 && <li className="empty">일치하는 항목이 없습니다</li>}
          </ul>
          {value.length > 0 && (
            <div className="search-select-foot muted small">
              {value.length}개 고름
              <button
                type="button"
                className="link"
                // 검색 칸에 포커스를 둔다(목록이 닫히지 않게).
                onMouseDown={(e) => e.preventDefault()}
                onClick={() => onChange([])}
              >
                모두 빼기
              </button>
            </div>
          )}
        </div>
      )}
    </div>
  );
}
