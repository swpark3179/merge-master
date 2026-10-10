import type { CellDecoration } from "@mm/grid";
import { countCells, SheetGrid } from "@mm/grid";
import type { Brush, CellRange, ColumnRule, HAlign, SheetModel, StylePatch } from "@mm/shared";
import { colLetters, FLAG, formatRange, formatValue, personColor, rgbHex } from "@mm/shared";
import type { ReactNode } from "react";
import { useCallback, useMemo, useRef, useState } from "react";

import { AssignmentPanel, ColumnPanel, ExceptionsPanel, PermissionsPanel } from "./panels";
import type { ConfigState } from "./useConfig";
import { useConfig } from "./useConfig";

interface Props {
  model: SheetModel;
  /** 화면이 보이는 중인지(다시 보일 때 설정을 새로 읽는다). */
  active: boolean;
}

const FONTS = ["맑은 고딕", "굴림", "돋움", "바탕", "Arial", "Calibri"];
const SIZES = [8, 9, 10, 11, 12, 14, 16, 18, 20, 24];

export function formatLabel(rule: ColumnRule | undefined): string {
  switch (rule?.format.kind) {
    case "number":
      return rule.format.unit ? `숫자(${rule.format.unit})` : "숫자";
    case "list":
      return "목록";
    case "date":
      return "날짜";
    case "text":
      return "글자";
    default:
      return "";
  }
}

/** 1b 취합 설정. */
export function ConfigScreen({ model: initial, active }: Props) {
  const s = useConfig(initial, active);
  const assignRef = useRef<HTMLSelectElement>(null);
  const [fmt, setFmt] = useState(false);
  const { model, view, flags, viewer } = s;
  const hr = model.headerRows;

  // 머리글 맨 아래 행(병합이면 그 병합의 왼쪽 위)에 열 태그(조회·배정 기준·필수·목록)를 붙인다.
  const headTagCells = useMemo(() => {
    const map = new Map<string, number>();
    if (hr === 0) return map;
    for (let c = 0; c < model.cols; c++) {
      const m = model.merges.find(
        (m) => m.row <= hr - 1 && hr - 1 < m.row + m.rows && m.col <= c && c < m.col + m.cols,
      );
      if (!m) map.set(`${hr - 1}:${c}`, c);
      else if (m.cols === 1) map.set(`${m.row}:${c}`, c);
    }
    return map;
  }, [model, hr]);

  const viewerIndex = viewer && view ? view.roster.indexOf(viewer) : -1;
  const rowMine = useMemo(() => {
    if (!view || viewerIndex < 0) return null;
    const out = new Uint8Array(model.rows);
    for (let r = 0; r < model.rows; r++) {
      if (view.rowOwner[r] === viewerIndex) out[r] = 1;
      else for (let c = 0; c < model.cols; c++) if (flags[r * model.cols + c]! & FLAG.MINE) out[r] = 1;
    }
    return out;
  }, [view, viewerIndex, flags, model]);

  const decorate = useCallback(
    (r: number, c: number): CellDecoration | undefined => {
      if (!view) return undefined;
      const f = flags[r * model.cols + c] ?? 0;
      const rule = view.config.columns[c];
      if (f & FLAG.HEADER) {
        const col = headTagCells.get(`${r}:${c}`);
        if (col === undefined || !rule) return undefined;
        const tags: ReactNode[] = [];
        if (rule.filter)
          tags.push(
            <span key="f" className="tag tag-filter">
              조회
            </span>,
          );
        if (view.config.assignment.keyCol === col)
          tags.push(
            <span key="k" className="tag tag-key">
              배정 기준
            </span>,
          );
        if (rule.required)
          tags.push(
            <span key="r" className="tag tag-required">
              필수
            </span>,
          );
        if (rule.format.kind === "list")
          tags.push(
            <span key="l" className="tag">
              목록
            </span>,
          );
        return tags.length ? { adornment: <span className="head-tags">{tags}</span> } : undefined;
      }
      if (rowMine && !rowMine[r]) return { className: "deco-others" };
      const cls: string[] = [];
      if (!(f & FLAG.INPUT)) cls.push("deco-locked");
      else if (rowMine && !(f & FLAG.MINE)) cls.push("deco-others");
      const v = model.cells[r * model.cols + c]?.v;
      if (f & FLAG.REQUIRED && (v === undefined || v === null || v === "")) cls.push("deco-missing");
      if (f & FLAG.EXCEPTION) cls.push("deco-exception");
      return cls.length ? { className: cls.join(" ") } : undefined;
    },
    [view, flags, model, headTagCells, rowMine],
  );

  const rowMarker = useCallback(
    (r: number) => {
      const p = view?.rowOwner[r] ?? -1;
      return p >= 0 ? personColor(p) : undefined;
    },
    [view],
  );

  // '남의 행 숨김' 시점이면 그 행을 크기 0으로 둔다(그리드는 크기 0인 행을 그리지 않는다).
  const shown = useMemo(() => {
    if (!view || view.hiddenRows.length === 0) return model;
    const rowHeights = [...model.rowHeights];
    for (const r of view.hiddenRows) rowHeights[r] = 0;
    return { ...model, rowHeights };
  }, [model, view]);

  return (
    <div className="screen config-screen">
      <div className="config-main">
        <RoleBar s={s} onAssign={() => assignRef.current?.focus()} />
        <div className="grid-card">
          <div className="grid-card-head">
            <span className="muted">
              {model.sheet} · {(model.rows - hr).toLocaleString()}행 {model.cols}열
            </span>
            <span className="spacer" />
            <ViewerPick s={s} />
            <button className={`ghost small${fmt ? " on" : ""}`} aria-pressed={fmt} onClick={() => setFmt((v) => !v)}>
              서식 편집
            </button>
          </div>
          {fmt && <FormatBar s={s} />}
          <FormulaBar s={s} />
          {viewer && view && <ViewerBanner s={s} />}
          <div className="config-grid">
            {view ? (
              <SheetGrid
                model={shown}
                decorate={decorate}
                rowMarker={rowMarker}
                selection={s.selection}
                onSelectionChange={s.setSelection}
                className="preview-grid"
              />
            ) : (
              <div className="placeholder muted">설정을 불러오는 중…</div>
            )}
          </div>
        </div>
        <Legend s={s} />
      </div>
      {view && (
        <aside className="config-side" aria-label="설정 패널">
          <ColumnPanel s={s} />
          <ExceptionsPanel s={s} />
          <AssignmentPanel s={s} selectRef={assignRef} />
          <PermissionsPanel s={s} />
        </aside>
      )}
    </div>
  );
}

function activeStyle(s: ConfigState) {
  const a = s.selection?.anchor;
  if (!a) return undefined;
  const cell = s.model.cells[a.row * s.model.cols + a.col];
  return s.model.styles[cell?.s ?? 0];
}

function FormatBar({ s }: { s: ConfigState }) {
  const range = s.range;
  const st = activeStyle(s);
  const disabled = !range || s.busy;
  const style = (patch: StylePatch) => range && void s.format({ op: "style", range, patch });
  const exactMerge = range && s.model.merges.some((m) => formatRange(m) === formatRange(range));
  return (
    <div className="toolbar" role="toolbar" aria-label="서식">
      <select
        aria-label="글꼴"
        disabled={disabled}
        value={st?.fontName || "맑은 고딕"}
        onChange={(e) => style({ fontName: e.target.value })}
      >
        {[...new Set([st?.fontName || "맑은 고딕", ...FONTS])].map((f) => (
          <option key={f}>{f}</option>
        ))}
      </select>
      <select
        aria-label="글꼴 크기"
        disabled={disabled}
        value={Math.round(st?.fontSize || 11)}
        onChange={(e) => style({ fontSize: Number(e.target.value) })}
      >
        {[...new Set([Math.round(st?.fontSize || 11), ...SIZES])]
          .sort((a, b) => a - b)
          .map((n) => (
            <option key={n}>{n}</option>
          ))}
      </select>
      <span className="sep" />
      <button
        className={`icon${st?.bold ? " on" : ""}`}
        aria-label="굵게"
        disabled={disabled}
        onClick={() => style({ bold: !st?.bold })}
      >
        <b>B</b>
      </button>
      <button
        className={`icon${st?.italic ? " on" : ""}`}
        aria-label="기울임"
        disabled={disabled}
        onClick={() => style({ italic: !st?.italic })}
      >
        <i>I</i>
      </button>
      <button
        className={`icon${st?.underline ? " on" : ""}`}
        aria-label="밑줄"
        disabled={disabled}
        onClick={() => style({ underline: !st?.underline })}
      >
        <u>U</u>
      </button>
      <label className="color-pick" title="글자색">
        <span style={{ borderBottom: `3px solid ${rgbHex(st?.color ?? 0)}` }}>가</span>
        <input
          type="color"
          aria-label="글자색"
          disabled={disabled}
          value={rgbHex(st?.color ?? 0)}
          onChange={(e) => style({ color: parseInt(e.target.value.slice(1), 16) })}
        />
      </label>
      <label className="color-pick" title="채우기">
        <span className="swatch" style={{ background: st?.fill !== undefined ? rgbHex(st.fill) : "#fff" }} />
        채우기
        <input
          type="color"
          aria-label="채우기 색"
          disabled={disabled}
          value={st?.fill !== undefined ? rgbHex(st.fill) : "#ffffff"}
          onChange={(e) => style({ fill: { kind: "color", rgb: parseInt(e.target.value.slice(1), 16) } })}
        />
      </label>
      <button className="small ghost" disabled={disabled} onClick={() => style({ fill: { kind: "none" } })}>
        채우기 없음
      </button>
      <select
        aria-label="테두리"
        disabled={disabled}
        value=""
        onChange={(e) => style({ borders: e.target.value as "all" | "outline" | "none" })}
      >
        <option value="" disabled>
          테두리 ▾
        </option>
        <option value="all">모든 테두리</option>
        <option value="outline">바깥쪽 테두리</option>
        <option value="none">테두리 없음</option>
      </select>
      <span className="sep" />
      <button
        className="ghost small"
        disabled={disabled}
        onClick={() => range && void s.format({ op: exactMerge ? "unmerge" : "merge", range })}
      >
        {exactMerge ? "병합 해제" : "셀 병합"}
      </button>
      <select
        aria-label="맞춤"
        disabled={disabled}
        value={st?.hAlign ?? "general"}
        onChange={(e) => style({ hAlign: e.target.value as HAlign })}
      >
        <option value="general">≡ 일반</option>
        <option value="left">왼쪽</option>
        <option value="center">가운데</option>
        <option value="right">오른쪽</option>
      </select>
    </div>
  );
}

function ViewerPick({ s }: { s: ConfigState }) {
  return (
    <label className="viewer-pick">
      참여자 시점으로 보기
      <select
        aria-label="참여자 시점"
        value={s.viewer ?? ""}
        disabled={s.busy}
        onChange={(e) => void s.setViewer(e.target.value || null)}
      >
        <option value="">선택 안 함</option>
        {s.view?.roster.map((p) => (
          <option key={p}>{p}</option>
        ))}
      </select>
    </label>
  );
}

const BRUSHES: { brush: Brush; name: string; label: ReactNode; title: string }[] = [
  {
    brush: "input",
    name: "입력 영역",
    label: (
      <>
        <span className="role-sw input" />
        입력으로 지정
      </>
    ),
    title: "참여자가 쓸 수 있는 칸으로",
  },
  {
    brush: "required",
    name: "필수",
    label: (
      <>
        <span className="role-sw req" />
        필수로 지정
      </>
    ),
    title: "반드시 채워야 하는 칸으로(다시 누르면 해제)",
  },
  {
    brush: "locked",
    name: "입력 금지",
    label: (
      <>
        <span className="role-sw lock" />
        잠금으로 지정
      </>
    ),
    title: "보기만 가능한 칸으로",
  },
  {
    brush: "filter",
    name: "조회조건",
    label: (
      <>
        <span className="tag tag-filter">조회</span>조회조건
      </>
    ),
    title: "열을 참여자 화면의 조회조건(드롭다운 필터)으로",
  },
];

/** 범위(열 머리글 = 데이터 행 전체)를 고르고 역할을 누른다. */
function RoleBar({ s, onAssign }: { s: ConfigState; onAssign: () => void }) {
  const disabled = !s.range || s.busy;
  const brush = (b: Brush) => s.range && void s.apply({ op: "brush", range: s.range, brush: b });
  const r = s.range;
  const hint = !r
    ? "열 머리글(A, B, C…)을 누르거나 범위를 끌어 고르세요. Shift로 여러 열을 함께 고를 수 있어요."
    : r.row <= s.model.headerRows && r.row + r.rows >= s.model.rows
      ? `${colSpan(s.model, r)}열을 골랐어요.`
      : `${rangeLabel(s.model, r)}을 골랐어요.`;
  return (
    <div className="role-bar" role="toolbar" aria-label="역할 지정">
      <span className="role-hint">{hint}</span>
      {BRUSHES.map((b) => (
        <button
          key={b.brush}
          className="ghost role"
          aria-label={b.name}
          title={b.title}
          disabled={disabled}
          onClick={() => brush(b.brush)}
        >
          {b.label}
        </button>
      ))}
      <button className="ghost role" aria-label="담당자 배정" disabled={disabled} onClick={onAssign}>
        담당자 배정
      </button>
      <button className="ghost role eraser" aria-label="지우개" disabled={disabled} onClick={() => brush("erase")}>
        지우개
      </button>
    </div>
  );
}

function colSpan(model: SheetModel, r: CellRange): string {
  const a = colLetters(model.origin.col + r.col);
  return r.cols === 1 ? a : `${a}–${colLetters(model.origin.col + r.col + r.cols - 1)}`;
}

function rangeLabel(model: SheetModel, r: CellRange): string {
  return formatRange({ ...r, row: r.row + model.origin.row, col: r.col + model.origin.col });
}

function FormulaBar({ s }: { s: ConfigState }) {
  const { range, model, view } = s;
  let summary = "";
  if (range && view) {
    const n = countCells(s.layout, range);
    const a = s.selection!.anchor;
    if (n === 1) {
      const cell = model.cells[a.row * model.cols + a.col];
      summary = cell?.f ?? formatValue(cell?.v, model.styles[cell?.s ?? 0]?.numberFormat ?? "");
    } else {
      const parts = [`${n.toLocaleString()}개 셀 선택됨`];
      if (range.cols === 1) {
        const rule = view.config.columns[range.col];
        if (rule) {
          parts.push(rule.role === "input" ? "입력 영역" : "입력 금지");
          if (rule.required) parts.push("필수");
          parts.push(formatLabel(rule));
        }
      }
      summary = parts.join(" · ");
    }
  }
  return (
    <div className="formula-bar">
      <span className="addr">{range ? rangeLabel(model, range) : ""}</span>
      <span className="fx">fx</span>
      <span className="value">{summary}</span>
      {s.error && <span className="error small">{s.error}</span>}
    </div>
  );
}

function ViewerBanner({ s }: { s: ConfigState }) {
  const view = s.view!;
  const p = view.progress.byPerson.find((x) => x.name === s.viewer);
  let mine = 0;
  for (const f of s.flags) if (f & FLAG.MINE) mine++;
  return (
    <div className="viewer-banner">
      <b>{s.viewer} 님 시점</b> 미리보기 · 입력 가능 {mine.toLocaleString()}칸 · 필수 {p?.filled ?? 0}/
      {p?.required ?? 0} · 남의 행 {view.config.permissions.othersRows === "hidden" ? "숨김" : "읽기 전용"}
      <span className="spacer" />
      <button className="ghost small" onClick={() => void s.setViewer(null)}>
        시점 끄기
      </button>
    </div>
  );
}

function Legend({ s }: { s: ConfigState }) {
  const l = s.view?.legend;
  const unassigned = s.view?.progress.unassigned ?? 0;
  const perms = s.view?.config.permissions;
  return (
    <footer className="legend">
      {l && (
        <>
          <span className="legend-item">
            <span className="role-sw input" />
            <b>입력</b>
            <span className="muted">참여자가 쓸 수 있음 · {l.inputCols}열</span>
          </span>
          <span className="legend-item">
            <span className="role-sw req" />
            <b>필수</b>
            <span className="muted">반드시 채워야 함 · 비어 있음 {l.requiredMissing.toLocaleString()}</span>
          </span>
          <span className="legend-item">
            <span className="role-sw lock" />
            <b>잠금</b>
            <span className="muted">보기만 가능 · {l.lockedCols}열</span>
          </span>
          {l.filterCols > 0 && (
            <span className="legend-item">
              <span className="tag tag-filter">조회</span>
              <span className="muted">조회조건 {l.filterCols}열</span>
            </span>
          )}
          {l.exceptions > 0 && (
            <span className="legend-item">
              <span className="chip-swatch exception" />
              <span className="muted">예외 {l.exceptions}</span>
            </span>
          )}
          {unassigned > 0 && <span className="warn">배정되지 않은 필수 {unassigned.toLocaleString()}칸</span>}
        </>
      )}
      <span className="spacer" />
      {perms && (
        <button
          className="switch"
          role="switch"
          aria-checked={perms.addRows}
          disabled={s.busy}
          onClick={() => void s.apply({ op: "setPermissions", permissions: { ...perms, addRows: !perms.addRows } })}
        >
          참여자가 새 행을 추가할 수 있게 하기
          <span className="track">
            <span className="knob" />
          </span>
        </button>
      )}
    </footer>
  );
}
