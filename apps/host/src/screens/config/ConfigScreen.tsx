import type { CellDecoration, GridHit, SheetGridHandle } from "@mm/grid";
import { countCells, selectForMenu, SheetGrid, withHiddenRows } from "@mm/grid";
import type { Axis, Brush, CellRange, ColumnRule, HAlign, SheetModel, StylePatch } from "@mm/shared";
import { colLetters, FLAG, formatRange, formatValue, parseHex, personColor, rgbHex } from "@mm/shared";
import type { MouseEvent, ReactNode } from "react";
import { useCallback, useMemo, useRef, useState } from "react";

import { useToast } from "../../frame";
import type { GridMenuState } from "../../GridMenu";
import { GridMenu } from "../../GridMenu";
import { SheetTabs } from "../../SheetTabs";
import { sizeEdits } from "../../sizes";
import { useHostEditor } from "../../useHostEditor";
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

/** 1b 취합 설정: 위쪽 탭(열 역할 · 담당자 배정), 그리드(칸 편집·크기 조절)와 그 아래 시트 탭, 두 탭 공통인 행 추가
 * 스위치, 오른쪽 상세. 열 역할·셀 예외·담당자 배정·참여자 권한은 시트마다, 제목·안내·마감·직접 넣은 명단은 모든 시트가
 * 같이 쓴다. */
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
  const onResize = async (axis: Axis, index: number, size: number) => {
    for (const edit of sizeEdits(shown, s.range, axis, index, size)) await s.format(edit);
  };

  // 오른쪽 클릭: 크기·행·열 추가·지우기·취소 행. 누른 곳이 지금 선택 밖이면 그 칸·열·행을 고른다(Excel처럼).
  const [menu, setMenu] = useState<GridMenuState | null>(null);
  const onContextMenu = (hit: GridHit, e: MouseEvent) => {
    e.preventDefault();
    editor.leave();
    const sel = selectForMenu(model, s.selection, hit);
    if (sel !== s.selection) s.setSelection(sel);
    setMenu({ x: e.clientX, y: e.clientY, hit });
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

  // '남의 행 숨김' 시점이면 그 행을 숨긴 행(크기 0)으로 둔다(그리드는 크기 0인 행을 그리지 않고, 크기 조절도 건너뛴다).
  const shown = useMemo(
    () => (view && view.hiddenRows.length > 0 ? withHiddenRows(model, new Set(view.hiddenRows)) : model),
    [model, view],
  );

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
              title="칸을 두 번 누르거나 바로 입력해 내용을 고치고, Delete로 지웁니다. 열·행 머리글 경계를 끌거나 머리글을 오른쪽 클릭하면 너비·높이가 바뀝니다. 오른쪽 클릭으로 표 끝에 행·열을 추가하고, 앱에서 추가한 행·열을 지웁니다."
            >
              두 번 눌러 고치기 · 머리글 경계를 끌어 크기 조절 · 오른쪽 클릭 메뉴
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
                onResize={(axis, index, size) => void onResize(axis, index, size)}
                onContextMenu={onContextMenu}
                className="preview-grid"
              />
            ) : (
              <div className="placeholder muted">설정을 불러오는 중…</div>
            )}
          </div>
          {tabs}
        </div>
        {menu && view && (
          <GridMenu
            menu={menu}
            model={model}
            shown={shown}
            view={view}
            selection={s.selection}
            onClose={() => setMenu(null)}
            onConfirm={(confirm) => setMenu({ ...menu, confirm })}
            onResize={(axis, index, size) => void onResize(axis, index, size)}
            onStructure={(edit) => void s.structure(edit)}
          />
        )}
        <footer className="config-foot">
          <AddRowsSwitch s={s} />
        </footer>
      </div>
      {view && (
        <aside className="config-side" aria-label="설정 패널">
          {tab === "roles" ? (
            <>
              <ColumnPanel s={s} />
              <PermissionsPanel s={s} />
              <ExceptionsPanel s={s} />
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
      <FillHex
        fill={st?.fill}
        range={range}
        disabled={disabled}
        onApply={(r, rgb) => void s.format({ op: "style", range: r, patch: { fill: { kind: "color", rgb } } })}
      />
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

/**
 * 채우기 색 코드(`#ff00ff`): 고른 칸(활성 칸)의 채우기를 보이고, 고쳐 Enter·붙여넣기·칸 밖으로 나가면 고른 범위에 칠한다.
 * 한 칸의 코드를 복사해 다른 칸을 고르고 붙여넣으면 같은 색이 된다. 고치기 시작할 때 고른 범위에 칠한다(다른 칸을 눌러
 * 나가도 그 칸에 칠하지 않게).
 */
function FillHex({
  fill,
  range,
  disabled,
  onApply,
}: {
  fill: number | undefined;
  range: CellRange | null;
  disabled: boolean;
  onApply: (range: CellRange, rgb: number) => void;
}) {
  const shown = fill !== undefined ? rgbHex(fill) : "";
  /** 고치는 중인 글과 그때 고른 범위(고치지 않으면 `null`). */
  const [draft, setDraft] = useState<{ text: string; range: CellRange } | null>(null);
  const [invalid, setInvalid] = useState(false);
  const commit = (text: string, target: CellRange | null) => {
    const rgb = parseHex(text);
    setDraft(null);
    setInvalid(false);
    if (rgb !== null && target) onApply(target, rgb); // 범위의 다른 칸은 색이 다를 수 있어 같은 색이어도 칠한다
  };
  return (
    <input
      className={`hex-input mono${invalid ? " invalid" : ""}`}
      aria-label="채우기 색 코드"
      aria-invalid={invalid}
      title="채우기 색 코드 — 복사해 다른 칸을 고른 뒤 붙여넣으면 같은 색이 됩니다"
      placeholder={disabled ? "" : "없음"}
      spellCheck={false}
      maxLength={9}
      disabled={disabled}
      value={draft?.text ?? shown}
      onFocus={(e) => e.currentTarget.select()}
      onChange={(e) => {
        setInvalid(false);
        if (range) setDraft({ text: e.target.value, range: draft?.range ?? range });
      }}
      onPaste={(e) => {
        const text = e.clipboardData.getData("text");
        if (parseHex(text) === null) return; // 색 코드가 아니면 그냥 붙여 넣는다(Enter로 확인)
        e.preventDefault();
        commit(text, draft?.range ?? range);
        e.currentTarget.select();
      }}
      onKeyDown={(e) => {
        if (e.key === "Enter" && !e.nativeEvent.isComposing) {
          if (!draft) return;
          if (parseHex(draft.text) === null) return setInvalid(true);
          commit(draft.text, draft.range);
          e.currentTarget.select();
        } else if (e.key === "Escape") {
          setDraft(null);
          setInvalid(false);
          e.currentTarget.blur();
        }
      }}
      onBlur={() => {
        if (draft) commit(draft.text, draft.range); // 색 코드가 아니면 원래대로
      }}
    />
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

/**
 * 열 역할 줄(열 역할은 여기서만 정한다): 칸이나 열을 고르고 입력/잠금 중 하나를 고르거나 조회조건을 켜고 끈다. 일부 칸만
 * 골라도 **고른 열 전체**에 적용한다 — 칸 단위 입력 가능·입력 금지는 오른쪽 '셀 예외'에서만 정한다. 필수·형식·안내는 오른쪽
 * 열 속성에서.
 */
function RoleBar({ s }: { s: ConfigState }) {
  const disabled = !s.range || s.busy;
  const brush = (b: Brush) => s.range && void s.apply({ op: "brush", range: s.range, brush: b });
  const r = s.range;
  const whole = r !== null && r.row <= s.model.headerRows && r.row + r.rows >= s.model.rows;
  // 고른 열들의 역할이 모두 같으면 그 역할을 켠 것으로 보인다(셀 예외는 따로 — 오른쪽 '셀 예외').
  const cols = r ? s.view!.config.columns.slice(r.col, r.col + r.cols) : [];
  const role = cols.length > 0 && cols.every((c) => c.role === cols[0]!.role) ? cols[0]!.role : null;
  // 조회조건: 모두 켜짐 · 일부만(누르면 모두 켠다) · 꺼짐.
  const filtered = cols.filter((c) => c.filter).length;
  const filter = filtered === 0 ? false : filtered === cols.length ? true : "mixed";
  const hint = !r
    ? "열 머리글(A, B, C…)이나 칸을 고르세요. 입력/잠금·조회조건은 고른 열 전체에 적용돼요. Shift로 여러 열을 함께 고를 수 있어요."
    : whole
      ? `${colSpan(s.model, r)}열을 골랐어요. 필수·형식은 오른쪽 열 속성에서 정해요.`
      : `${rangeLabel(s.model, r)}을 골랐어요. 입력/잠금은 ${colSpan(s.model, r)}열 전체에 적용돼요. 이 칸만 바꾸려면 오른쪽 셀 예외에서.`;
  return (
    <div className="role-bar" role="toolbar" aria-label="역할 지정">
      <span className="role-hint">{hint}</span>
      <div className="role-toggle" role="group" aria-label="입력 또는 잠금">
        <button
          className="input"
          aria-pressed={role === "input"}
          title="고른 열 전체를 참여자가 쓸 수 있는 열로"
          disabled={disabled}
          onClick={() => brush("input")}
        >
          <RoleIcon kind="input" />
          입력
        </button>
        <button
          className="lock"
          aria-pressed={role === "locked"}
          title="고른 열 전체를 보기만 가능한 열로"
          disabled={disabled}
          onClick={() => brush("locked")}
        >
          <RoleIcon kind="lock" />
          잠금
        </button>
      </div>
      <button
        className="filter-toggle"
        aria-pressed={filter}
        title={
          filter === "mixed"
            ? "고른 열 중 일부만 조회조건이에요. 누르면 모두 조회조건으로"
            : "열을 참여자 화면의 조회조건(드롭다운 필터)으로(다시 누르면 해제)"
        }
        disabled={disabled}
        onClick={() => brush("filter")}
      >
        <RoleIcon kind="filter" />
        조회조건
        <span className="filter-state" aria-hidden>
          {filter === true ? "켬" : filter === "mixed" ? "일부" : "끔"}
        </span>
      </button>
    </div>
  );
}

/** 역할 줄 아이콘(16×16 선, 글자색을 따른다). */
function RoleIcon({ kind }: { kind: "input" | "lock" | "filter" }) {
  return (
    <svg
      className="role-icon"
      viewBox="0 0 16 16"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.5"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden
    >
      {kind === "input" ? (
        <path d="M10.5 2.5l3 3L6 13H3v-3zM9 4l3 3" />
      ) : kind === "lock" ? (
        <>
          <rect x="3" y="7" width="10" height="7" rx="1.5" />
          <path d="M5.5 7V5a2.5 2.5 0 0 1 5 0v2" />
        </>
      ) : (
        <path d="M2.5 3h11L9 8.5V13l-2-1V8.5z" />
      )}
    </svg>
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
      {p?.required ?? 0} · 남의 행{" "}
      {{ hidden: "숨김", editable: "편집 가능", readOnly: "읽기 전용" }[view.config.permissions.othersRows]}
      <span className="spacer" />
      <button className="ghost small" onClick={() => void s.setViewer(null)}>
        시점 끄기
      </button>
    </div>
  );
}

/** 그리드 아래 오른쪽(두 탭 공통): 참여자 행 추가 허용. 참여자는 자기가 추가한 행을 늘 지울 수 있다. */
function AddRowsSwitch({ s }: { s: ConfigState }) {
  const perms = s.view?.config.permissions;
  if (!perms) return null;
  return (
    <button
      className="switch"
      role="switch"
      aria-checked={perms.addRows}
      disabled={s.busy}
      title="켜면 참여자 화면의 마지막 행 번호 아래에 ＋가 생겨 표 맨 아래에 행을 붙일 수 있습니다. 참여자는 자기가 추가한 행을 행 번호 오른쪽 클릭으로 지울 수 있습니다"
      onClick={() => void s.apply({ op: "setPermissions", permissions: { ...perms, addRows: !perms.addRows } })}
    >
      참여자가 새 행을 추가할 수 있게 하기
      <span className="track">
        <span className="knob" />
      </span>
    </button>
  );
}
