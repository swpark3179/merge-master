import type { CellDecoration, SheetGridHandle } from "@mm/grid";
import { countCells, SheetGrid } from "@mm/grid";
import type { Brush, CellRange, ColumnRule, FormatEdit, HAlign, SheetModel, StylePatch } from "@mm/shared";
import { colLetters, FLAG, formatRange, formatValue, personColor, rgbHex } from "@mm/shared";
import type { ReactNode } from "react";
import { useCallback, useMemo, useRef, useState } from "react";

import { useToast } from "../../frame";
import { SheetTabs } from "../../SheetTabs";
import { useHostEditor } from "../../useHostEditor";
import { cellsRole, rangeAccess } from "./access";
import {
  AssignBar,
  ColumnPanel,
  ExceptionsPanel,
  PermissionsPanel,
  RangeAssignmentsPanel,
  RosterPanel,
} from "./panels";
import type { ConfigState } from "./useConfig";
import { useConfig } from "./useConfig";

type Tab = "roles" | "assign";

interface Props {
  /** 확정한 시트들(세션 안 순번 순서). */
  models: SheetModel[];
  /** 보고 있는 시트. 다른 단계(모니터링 등)와 같이 쓴다. */
  sheet: number;
  onSheet: (sheet: number) => void;
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

/** 1b 취합 설정: 위쪽 탭(열 역할 · 담당자 배정), 그리드(칸 편집·크기 조절)와 그 아래 시트 탭, 오른쪽 상세.
 * 열 역할·셀 예외·담당자 배정·참여자 권한은 시트마다, 제목·안내·마감·직접 넣은 명단은 모든 시트가 같이 쓴다. */
export function ConfigScreen({ models, sheet, onSheet, active }: Props) {
  const at = Math.min(sheet, models.length - 1);
  const tabs = <SheetTabs names={models.map((m) => m.sheet)} active={at} onSelect={onSheet} />;
  // 시트를 바꾸면 선택·시점·편집기를 새로 시작한다(시트마다 좌표가 다르다).
  return <SheetConfig key={at} initial={models[at]!} sheet={at} active={active} tabs={tabs} />;
}

function SheetConfig({
  initial,
  sheet,
  active,
  tabs,
}: {
  initial: SheetModel;
  sheet: number;
  active: boolean;
  tabs: ReactNode;
}) {
  const s = useConfig(initial, sheet, active);
  const [fmt, setFmt] = useState(false);
  const [tab, setTab] = useState<Tab>("roles");
  const grid = useRef<SheetGridHandle>(null);
  const gridHost = useRef<HTMLDivElement>(null);
  const toast = useToast();
  const { model, view, flags, viewer } = s;
  const hr = model.headerRows;

  const { setCells } = s;
  const commitCells = useCallback((cells: Parameters<typeof setCells>[0]) => void setCells(cells), [setCells]);
  const editor = useHostEditor({
    model,
    flags,
    columns: view?.config.columns,
    grid,
    host: gridHost,
    selection: s.selection,
    setSelection: s.setSelection,
    commit: commitCells,
    notify: toast,
  });

  // 머리글로 여러 열(행)을 골라 두고 그중 하나의 경계를 끌면 고른 열(행) 모두를 같은 크기로(Excel처럼).
  const onResize = (axis: "col" | "row", index: number, size: number) => {
    const r = s.range;
    let from = index;
    let count = 1;
    if (
      r &&
      axis === "col" &&
      r.row <= hr &&
      r.row + r.rows >= model.rows &&
      index >= r.col &&
      index < r.col + r.cols
    ) {
      from = r.col;
      count = r.cols;
    } else if (
      r &&
      axis === "row" &&
      r.col === 0 &&
      r.cols === model.cols &&
      index >= r.row &&
      index < r.row + r.rows
    ) {
      from = r.row;
      count = r.rows;
    }
    const edit: FormatEdit =
      axis === "col" ? { op: "colWidth", from, count, width: size } : { op: "rowHeight", from, count, height: size };
    void s.format(edit);
  };

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

  const keyCol = view?.config.assignment.keyCol;
  return (
    <div className="screen config-screen">
      <div className="config-main">
        <div className="config-tabs" role="tablist" aria-label="설정 종류">
          <button role="tab" aria-selected={tab === "roles"} onClick={() => setTab("roles")}>
            열 역할
          </button>
          <button role="tab" aria-selected={tab === "assign"} onClick={() => setTab("assign")}>
            담당자 배정
            <span className={`tag${keyCol !== undefined && keyCol !== null ? " tag-key" : ""}`}>
              {keyCol !== undefined && keyCol !== null
                ? `${colLetters(model.origin.col + keyCol)}열 기준 · ${view?.roster.length ?? 0}명`
                : view && view.roster.length > 0
                  ? `${view.roster.length}명`
                  : "배정 없음 · 누구나"}
            </span>
          </button>
        </div>
        {view && (tab === "roles" ? <RoleBar s={s} /> : <AssignBar s={s} />)}
        <div className="grid-card">
          <div className="grid-card-head">
            <span className="muted">
              {model.sheet} · {(model.rows - hr).toLocaleString()}행 {model.cols}열
            </span>
            <span
              className="muted small grid-tip"
              title="칸을 두 번 누르거나 바로 입력해 내용을 고치고, Delete로 지웁니다. 열·행 머리글 경계를 끌면 너비·높이가 바뀝니다."
            >
              두 번 눌러 고치기 · 머리글 경계를 끌어 크기 조절
            </span>
            <span className="spacer" />
            <ViewerPick s={s} />
            <button className={`ghost small${fmt ? " on" : ""}`} aria-pressed={fmt} onClick={() => setFmt((v) => !v)}>
              서식 편집
            </button>
          </div>
          {fmt && <FormatBar s={s} />}
          <FormulaBar s={s} editing={editor.editing?.text ?? null} />
          {viewer && view && <ViewerBanner s={s} />}
          <div className="config-grid" ref={gridHost}>
            {view ? (
              <SheetGrid
                handle={grid}
                model={shown}
                decorate={decorate}
                rowMarker={rowMarker}
                selection={s.selection}
                onSelectionChange={editor.onSelectionChange}
                overlay={editor.overlay}
                onKey={editor.onKey}
                onCellDoubleClick={editor.onCellDoubleClick}
                onResize={onResize}
                className="preview-grid"
              />
            ) : (
              <div className="placeholder muted">설정을 불러오는 중…</div>
            )}
          </div>
          {tabs}
        </div>
        {tab === "roles" ? <Legend s={s} /> : <AccessFooter s={s} />}
      </div>
      {view && (
        <aside className="config-side" aria-label="설정 패널">
          {tab === "roles" ? (
            <>
              <ColumnPanel s={s} />
              <ExceptionsPanel s={s} />
              <PermissionsPanel s={s} />
            </>
          ) : (
            <>
              <RosterPanel s={s} />
              <RangeAssignmentsPanel s={s} />
            </>
          )}
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

/** 열 역할 줄: 범위(열 머리글 = 데이터 행 전체)를 고르고 입력/잠금 중 하나를 고른다. 필수는 오른쪽 열 속성에서. */
function RoleBar({ s }: { s: ConfigState }) {
  const disabled = !s.range || s.busy;
  const brush = (b: Brush) => s.range && void s.apply({ op: "brush", range: s.range, brush: b });
  const r = s.range;
  const whole = r !== null && r.row <= s.model.headerRows && r.row + r.rows >= s.model.rows;
  // 열 전체를 골랐으면 그 열들의 역할, 일부 칸만 골랐으면 그 칸들의 실제 역할(셀 예외 반영)을 켠 것으로 보인다.
  const cols = r ? s.view!.config.columns.slice(r.col, r.col + r.cols) : [];
  const role = !r
    ? null
    : whole
      ? cols.length > 0 && cols.every((c) => c.role === cols[0]!.role)
        ? cols[0]!.role
        : null
      : (cellsRole(s.flags, s.model.cols, r) ??
        (cols.length > 0 && cols.every((c) => c.role === cols[0]!.role) ? cols[0]!.role : null));
  const hint = !r
    ? "열 머리글(A, B, C…)을 누르거나 범위를 끌어 고르세요. Shift로 여러 열을 함께 고를 수 있어요."
    : whole
      ? `${colSpan(s.model, r)}열을 골랐어요. 필수는 오른쪽 열 속성에서 정해요.`
      : `${rangeLabel(s.model, r)}을 골랐어요. 일부 칸만 바꾸면 셀 예외가 됩니다.`;
  return (
    <div className="role-bar" role="toolbar" aria-label="역할 지정">
      <span className="role-hint">{hint}</span>
      <div className="segmented role-seg" role="group" aria-label="입력 또는 잠금">
        <button
          className={role === "input" ? "on" : ""}
          aria-pressed={role === "input"}
          title="참여자가 쓸 수 있는 칸으로"
          disabled={disabled}
          onClick={() => brush("input")}
        >
          <span className="role-sw input" />
          입력
        </button>
        <button
          className={role === "locked" ? "on" : ""}
          aria-pressed={role === "locked"}
          title="보기만 가능한 칸으로"
          disabled={disabled}
          onClick={() => brush("locked")}
        >
          <span className="role-sw lock" />
          잠금
        </button>
      </div>
      <button
        className="ghost role"
        aria-label="조회조건"
        title="열을 참여자 화면의 조회조건(드롭다운 필터)으로(다시 누르면 해제)"
        disabled={disabled}
        onClick={() => brush("filter")}
      >
        <span className="tag tag-filter">조회</span>조회조건
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

function FormulaBar({ s, editing }: { s: ConfigState; editing: string | null }) {
  const { range, model, view } = s;
  let summary = "";
  if (editing !== null) {
    summary = editing;
  } else if (range && view) {
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
          parts.push(rule.role === "input" ? "입력" : "잠금");
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

/** '담당자 배정' 탭 그리드 아래: 고른 칸을 편집할 수 있는 사람(담당자가 없으면 누구나), 참여자 행 추가 스위치. */
function AccessFooter({ s }: { s: ConfigState }) {
  const { view, range, model } = s;
  const acc = view && range ? rangeAccess(view, s.flags, model.cols, range) : null;
  const inputCells = acc ? acc.cells - acc.locked : 0;
  return (
    <footer className="legend access-footer" aria-label="편집할 수 있는 사람">
      {!view ? null : !range || !acc ? (
        <span className="muted">칸을 고르면 그 칸을 편집할 수 있는 사람이 보입니다.</span>
      ) : (
        <>
          <span className="access-where">
            <span className="mono">{rangeLabel(model, range)}</span>
            {acc.cells > 1 && <span className="muted"> · {acc.cells.toLocaleString()}칸</span>}
          </span>
          <b>편집 가능</b>
          {inputCells === 0 ? (
            <span className="muted" data-testid="editors">
              없음 — 잠금 칸이라 참여자는 편집할 수 없습니다
            </span>
          ) : (
            <span className="editors" data-testid="editors">
              {acc.open > 0 && (
                <span className="person anyone" title="담당자가 없는 입력 칸은 누구나 편집할 수 있습니다">
                  누구나{acc.open < inputCells ? ` (${acc.open.toLocaleString()}칸)` : ""}
                </span>
              )}
              {acc.people.map((p) => (
                <span key={p} className="person">
                  <span className="mmg-dot" style={{ background: personColor(p) }} />
                  {view.roster[p]}
                </span>
              ))}
            </span>
          )}
          {acc.locked > 0 && inputCells > 0 && <span className="muted">잠금 {acc.locked.toLocaleString()}칸 제외</span>}
          {acc.required > 0 && <span className="tag tag-required">필수</span>}
        </>
      )}
      <span className="spacer" />
      <AddRowsSwitch s={s} />
    </footer>
  );
}

/** '열 역할' 탭 그리드 아래: 입력·필수·잠금 범례와 집계. */
function Legend({ s }: { s: ConfigState }) {
  const l = s.view?.legend;
  const unassigned = s.view?.progress.unassigned ?? 0;
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
          {unassigned > 0 && (
            <span className="muted" title="담당자가 아무도 없는 필수 칸은 누구나 입력할 수 있습니다">
              담당자 없는 필수 {unassigned.toLocaleString()}칸(누구나 입력)
            </span>
          )}
        </>
      )}
      <span className="spacer" />
      <AddRowsSwitch s={s} />
    </footer>
  );
}

/** 그리드 아래 오른쪽: 참여자 행 추가 허용(참여자 권한 '행 추가'와 같다). */
function AddRowsSwitch({ s }: { s: ConfigState }) {
  const perms = s.view?.config.permissions;
  if (!perms) return null;
  return (
    <button
      className="switch"
      role="switch"
      aria-checked={perms.addRows}
      disabled={s.busy}
      title="켜면 참여자 화면에 '+ 행 추가'가 생겨 표 맨 아래에 행을 붙일 수 있습니다"
      onClick={() => void s.apply({ op: "setPermissions", permissions: { ...perms, addRows: !perms.addRows } })}
    >
      참여자가 새 행을 추가할 수 있게 하기
      <span className="track">
        <span className="knob" />
      </span>
    </button>
  );
}
