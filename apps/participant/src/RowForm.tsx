import type { CellRef, CellValue, ColumnRule, InputFormat } from "@mm/shared";
import { a1, displayValue, editText, FLAG, normalize } from "@mm/shared";
import type { KeyboardEvent } from "react";
import { useEffect, useMemo, useRef, useState } from "react";

import { effectiveColumn, missingOf } from "./filters";
import type { SessionState } from "./useSession";

interface Props {
  s: SessionState;
  myRows: Uint8Array;
  row: number | null;
  onRow: (row: number) => void;
  onShowInGrid: (row: number) => void;
}

function clock(ms: number): string {
  const d = new Date(ms);
  return [d.getHours(), d.getMinutes()].map((n) => String(n).padStart(2, "0")).join(":");
}

/** 1g 행 단위 입력 폼: 내 담당 행 목록 · 읽기 전용 요약 띠 · 형식별 입력 · 칸 이탈 시 자동 저장 · 같은 비목 비교. */
export function RowForm({ s, myRows, row, onRow, onShowInGrid }: Props) {
  const w = s.welcome!;
  const sheet = s.sheet!;
  const model = sheet.model;
  const flags = s.flags;
  const { columns, labels } = s.view!;
  const [query, setQuery] = useState("");
  const [tab, setTab] = useState<"all" | "missing">("all");
  const hiddenRows = useMemo(() => new Set(model.hiddenRows), [model]);
  const hiddenCols = useMemo(() => new Set(model.hiddenCols), [model]);
  const flag = (r: number, c: number) => flags[r * model.cols + c] ?? 0;

  // 행을 부르는 이름: 조회조건 열(없으면 첫 입력 열 왼쪽의 입력 금지 글자 열).
  const labelCols = useMemo(() => {
    const filters = columns.map((c, i) => (c.filter ? i : -1)).filter((i) => i >= 0);
    if (filters.length > 0) return filters;
    const firstInput = columns.findIndex((c) => c.role === "input");
    return columns.map((_, i) => i).filter((i) => i < (firstInput < 0 ? 0 : firstInput));
  }, [columns]);
  const effective = useMemo(
    () => Array.from({ length: model.cols }, (_, c) => effectiveColumn(model, c)),
    // 값이 바뀌어도 이름 열(입력 금지)은 그대로라 시트가 바뀔 때만 다시 만든다.
    [model],
  );
  const show = (r: number, c: number, v: CellValue | undefined = effective[c]?.[r]) =>
    displayValue(v, model.styles[model.cells[r * model.cols + c]?.s ?? 0]?.numberFormat ?? "", columns[c]?.format);
  const rowLabel = (r: number) =>
    [...labelCols]
      .reverse()
      .map((c) => show(r, c))
      .filter(Boolean)
      .slice(0, 2)
      .join(" · ");

  const rows = useMemo(() => {
    const out: number[] = [];
    for (let r = model.headerRows; r < model.rows; r++) if (myRows[r] && !hiddenRows.has(r)) out.push(r);
    return out;
  }, [model, myRows, hiddenRows]);

  // 목록은 탭·검색을 바꿀 때 정렬한다(입력하는 동안 순서가 바뀌지 않게). 미입력 행을 위로.
  const listed = useMemo(() => {
    const q = query.trim();
    return rows
      .map((r) => ({ r, missing: missingOf(model, flags, r), label: rowLabel(r) }))
      .filter(
        (x) =>
          (tab === "all" || x.missing > 0) &&
          (!q || x.label.includes(q) || String(model.origin.row + x.r + 1).includes(q)),
      )
      .sort((a, b) => Number(b.missing > 0) - Number(a.missing > 0) || a.r - b.r);
    // 값이 바뀔 때는 다시 정렬하지 않는다(의존 목록에 값을 넣지 않음).
  }, [rows, tab, query]);

  const missingRows = rows.filter((r) => missingOf(model, flags, r) > 0).length;
  const current = row !== null && rows.includes(row) ? row : (listed[0]?.r ?? rows[0] ?? null);

  const { rowHistory } = s;
  useEffect(() => {
    if (current !== null) rowHistory(current);
  }, [current, sheet.myChanges, rowHistory, s.active]);

  const fieldRefs = useRef<(HTMLElement | null)[]>([]);

  if (current === null) {
    return <div className="form-empty muted">내 담당 행이 없습니다. 호스트에게 배정을 확인해 주세요.</div>;
  }

  const visibleCols = Array.from({ length: model.cols }, (_, c) => c).filter(
    (c) => !hiddenCols.has(c) && !(flag(current, c) & FLAG.COVERED),
  );
  const fieldCols = visibleCols.filter((c) => flag(current, c) & FLAG.MINE);
  const summaryCols = visibleCols.filter((c) => !(flag(current, c) & FLAG.MINE));
  const required = fieldCols.filter((c) => flag(current, c) & FLAG.REQUIRED).length;
  const missing = missingOf(model, flags, current);
  const sheetRow = model.origin.row + current + 1;

  const go = (step: 1 | -1) => {
    const i = listed.findIndex((x) => x.r === current);
    const next = listed[(i + step + listed.length) % listed.length];
    if (next) onRow(next.r);
  };
  const nextMissing = () => {
    const after = rows.filter((r) => r > current).concat(rows.filter((r) => r < current));
    const r = after.find((x) => missingOf(model, flags, x) > 0);
    if (r !== undefined) onRow(r);
    else s.notify("미입력 행이 없습니다");
  };
  const nextRow = () => {
    const i = rows.indexOf(current);
    const r = rows[i + 1];
    if (r !== undefined) onRow(r);
  };

  // 같은 비목 비교: 행 이름의 마지막 열이 같은 행들, 옆 이름 열과 첫 필수 입력 열.
  const keyCol = labelCols[labelCols.length - 1];
  const sideCol = labelCols.length > 1 ? labelCols[labelCols.length - 2] : undefined;
  const valueCol = fieldCols.find((c) => flag(current, c) & FLAG.REQUIRED) ?? fieldCols[0];
  const peers =
    keyCol !== undefined && valueCol !== undefined
      ? Array.from({ length: model.rows }, (_, r) => r)
          .filter((r) => r >= model.headerRows && !hiddenRows.has(r) && show(r, keyCol) === show(current, keyCol))
          .slice(0, 12)
      : [];

  const history =
    s.history?.sheet === s.active && s.history.row === current ? s.history.entries.slice(-3).reverse() : [];

  return (
    <div className="row-form">
      <aside className="row-list" aria-label="내 담당 행">
        <div className="row-list-head">
          <b>내 담당 행</b>
          <span className="muted">{rows.length.toLocaleString()}행</span>
        </div>
        <input
          className="row-search"
          aria-label="행 검색"
          placeholder="행 검색 (비목, 부서…)"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
        />
        <div className="row-tabs" role="tablist">
          <button role="tab" aria-selected={tab === "all"} onClick={() => setTab("all")}>
            전체 {rows.length}
          </button>
          <button role="tab" aria-selected={tab === "missing"} onClick={() => setTab("missing")}>
            미입력 {missingRows}
          </button>
        </div>
        <ul>
          {listed.slice(0, 300).map((x) => {
            const left = missingOf(model, flags, x.r);
            return (
              <li key={x.r}>
                <button className={`row-item${x.r === current ? " on" : ""}`} onClick={() => onRow(x.r)}>
                  <span className="mono">{model.origin.row + x.r + 1}</span>
                  <span className="row-item-label">{x.label || "–"}</span>
                  {left > 0 ? <span className="tag tag-required">미입력 {left}</span> : <span className="ok">✓</span>}
                </button>
              </li>
            );
          })}
          {listed.length > 300 && <li className="muted small">⋮ {listed.length - 300}행 더 (검색으로 찾기)</li>}
        </ul>
      </aside>

      <section className="row-main" aria-label={`${sheetRow}행 입력`}>
        <div className="row-head">
          <b className="big">{sheetRow}행</b>
          <span className={missing > 0 ? "warn" : "ok"}>
            {missing > 0 ? `${missing} / ${required} 미입력` : "필수 입력 완료"}
          </span>
          <span className="muted">{rowLabel(current)}</span>
          <span className="spacer" />
          <button className="ghost small" onClick={() => go(-1)}>
            ‹ 이전
          </button>
          <button className="small" onClick={nextMissing}>
            다음 미입력 ›
          </button>
        </div>
        <div className="summary-band" aria-label="읽기 전용 값">
          {summaryCols.map((c) => (
            <span key={c} className="summary-item">
              <span className="muted">{labels[c] || `열 ${c + 1}`}</span>
              <b>
                {flag(current, c) & FLAG.FORMULA ? <span className="tag">수식</span> : null} {show(current, c) || "–"}
              </b>
            </span>
          ))}
        </div>
        <div className="fields">
          {fieldCols.map((c, i) => (
            <Field
              key={`${current}:${c}`}
              s={s}
              at={{ row: current, col: c }}
              label={labels[c] || `열 ${c + 1}`}
              address={a1(model.origin.row + current, model.origin.col + c)}
              rule={columns[c]}
              required={(flag(current, c) & FLAG.REQUIRED) !== 0}
              value={sheet.value(current, c)}
              locker={sheet.locks.get(current * model.cols + c)}
              readOnly={s.readOnly}
              inputRef={(el) => (fieldRefs.current[i] = el)}
              onEnter={() => fieldRefs.current[i + 1]?.focus()}
              onNextRow={nextRow}
            />
          ))}
        </div>
        <div className="row-foot muted small">
          칸을 벗어나면 자동 저장 · Enter 다음 칸 · Ctrl+Enter 다음 행
          {history.length > 0 && (
            <span className="row-history">
              {" "}
              · 이 행 변경 이력:{" "}
              {history.map((h) => (
                <span key={h.seq}>
                  {clock(h.atMs)} {labels[h.col] || a1(0, h.col)}{" "}
                  {h.old == null || h.old === "" ? "(빈 값)" : show(h.row, h.col, h.old)} →{" "}
                  {h.new == null || h.new === "" ? "(빈 값)" : show(h.row, h.col, h.new)}
                  {h.user !== w.name ? ` (${h.user})` : ""}
                  {"; "}
                </span>
              ))}
            </span>
          )}
        </div>
      </section>

      {peers.length > 1 && keyCol !== undefined && valueCol !== undefined && (
        <aside className="compare" aria-label="같은 비목 비교">
          <b>같은 {labels[keyCol] || "항목"} 비교</b>
          <div className="muted small">‘{show(current, keyCol)}’ 행을 옆에 두고 입력 (읽기 전용)</div>
          <table>
            <thead>
              <tr>
                <th>{sideCol !== undefined ? labels[sideCol] : "행"}</th>
                <th>{labels[valueCol]}</th>
              </tr>
            </thead>
            <tbody>
              {peers.map((r) => (
                <tr key={r} className={r === current ? "me" : undefined}>
                  <td>
                    {sideCol !== undefined ? show(r, sideCol) : model.origin.row + r + 1}
                    {r === current ? " (나)" : ""}
                  </td>
                  <td className="num">{show(r, valueCol, sheet.value(r, valueCol)) || "–"}</td>
                </tr>
              ))}
            </tbody>
          </table>
          <button className="link" onClick={() => onShowInGrid(current)}>
            그리드에서 이 행 보기 →
          </button>
        </aside>
      )}
    </div>
  );
}

interface FieldProps {
  s: SessionState;
  at: CellRef;
  label: string;
  address: string;
  rule: ColumnRule | undefined;
  required: boolean;
  value: CellValue;
  locker: string | undefined;
  readOnly: boolean;
  inputRef: (el: HTMLElement | null) => void;
  onEnter: () => void;
  onNextRow: () => void;
}

function Field({
  s,
  at,
  label,
  address,
  rule,
  required,
  value,
  locker,
  readOnly,
  inputRef,
  onEnter,
  onNextRow,
}: FieldProps) {
  const format: InputFormat = rule?.format ?? { kind: "text" };
  const [text, setText] = useState(() => editText(value, format));
  const [focused, setFocused] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const disabled = readOnly || locker !== undefined;

  // 다른 사람·서버가 바꾼 값: 입력 중이 아니고 고칠 오류가 없을 때만 따라간다.
  useEffect(() => {
    if (!focused && !error) setText(editText(value, format));
  }, [value, focused]);

  // 남이 먼저 잡았으면(잠금 거절) 이 칸을 떠난다.
  const { lockEvent } = s;
  useEffect(() => {
    if (
      lockEvent?.kind === "denied" &&
      lockEvent.sheet === s.active &&
      lockEvent.at.row === at.row &&
      lockEvent.at.col === at.col &&
      focused
    ) {
      (document.activeElement as HTMLElement | null)?.blur();
      s.notify(
        lockEvent.by ? `${lockEvent.by} 님이 입력 중입니다` : (lockEvent.reason ?? "입력할 수 없습니다"),
        "warn",
      );
    }
  }, [lockEvent]);

  const check = (t: string) => {
    const n = normalize(format, t);
    setError(n.ok ? null : n.error);
    return n;
  };

  const save = (t: string) => {
    const n = check(t);
    if (!n.ok || !s.edit([{ row: at.row, col: at.col, value: n.value }])) s.unlock(at);
  };

  const common = {
    id: `f-${at.row}-${at.col}`,
    disabled,
    "aria-invalid": error !== null,
    onFocus: () => {
      setFocused(true);
      s.lock(at);
    },
    onBlur: () => {
      setFocused(false);
      save(text);
    },
    onKeyDown: (e: KeyboardEvent) => {
      if (e.nativeEvent.isComposing || e.keyCode === 229 || e.key !== "Enter" || e.altKey) return;
      e.preventDefault();
      if (e.ctrlKey || e.metaKey) {
        save(text);
        onNextRow();
      } else onEnter();
    },
  };

  const empty = value === null || value === undefined || value === "";
  let control;
  if (format.kind === "list") {
    control = (
      <div className="chips" role="radiogroup" aria-label={label}>
        {format.items.map((item) => (
          <button
            key={item}
            role="radio"
            aria-checked={value === item}
            className={`chip${value === item ? " on" : ""}`}
            disabled={disabled}
            ref={item === format.items[0] ? inputRef : undefined}
            onClick={() => {
              setText(item);
              s.edit([{ row: at.row, col: at.col, value: item }]);
            }}
          >
            {item}
          </button>
        ))}
      </div>
    );
  } else if (format.kind === "text") {
    control = (
      <textarea
        {...common}
        ref={inputRef}
        rows={2}
        value={text}
        placeholder={rule?.hint || undefined}
        onChange={(e) => {
          setText(e.target.value);
          if (error) check(e.target.value);
        }}
      />
    );
  } else {
    control = (
      <span className="field-input">
        <input
          {...common}
          ref={inputRef}
          type={format.kind === "date" ? "date" : "text"}
          inputMode={format.kind === "number" ? "decimal" : undefined}
          value={text}
          onChange={(e) => {
            setText(e.target.value);
            check(e.target.value);
          }}
        />
        {format.kind === "number" && format.unit && <span className="unit">{format.unit}</span>}
      </span>
    );
  }

  return (
    <div className={`field${required && empty ? " missing" : ""}${disabled ? " disabled" : ""}`}>
      <label htmlFor={`f-${at.row}-${at.col}`}>
        <b>{label}</b>
        {required && <span className="req"> *</span>}
        <span className="mono muted small"> {address}</span>
      </label>
      {control}
      {locker ? (
        <div className="field-note lock">● {locker} 님이 편집 중 — 끝나면 입력할 수 있어요</div>
      ) : error ? (
        <div className="field-note error" role="alert">
          {error}
        </div>
      ) : required && empty ? (
        <div className="field-note warn">필수 입력 항목입니다{rule?.hint ? ` · ${rule.hint}` : ""}</div>
      ) : rule?.hint ? (
        <div className="field-note muted">{rule.hint}</div>
      ) : null}
    </div>
  );
}
