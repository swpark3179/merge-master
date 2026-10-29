import type { CellRange, CellValue, SheetModel } from "@mm/shared";
import { formatValue } from "@mm/shared";

/**
 * 행 필터 공용 도구(참여자 1f 조회조건, 호스트 1d 모니터링). 걸러진 데이터 행은 높이 0(숨김)으로 그린다.
 * 병합 영역은 그리드가 보이는 행만으로 다시 그리므로(높이 합) 따로 쪼개지 않아도 된다(왼쪽 위 칸이 걸러져도 값이 보인다).
 */

/** 병합을 고려한 열 값(병합 영역 안이면 왼쪽 위 칸의 값). */
export function effectiveColumn(model: SheetModel, col: number): (CellValue | undefined)[] {
  const out: (CellValue | undefined)[] = [];
  for (let r = 0; r < model.rows; r++) out.push(model.cells[r * model.cols + col]?.v);
  for (const m of model.merges) {
    if (col < m.col || col >= m.col + m.cols) continue;
    const v = model.cells[m.row * model.cols + m.col]?.v;
    for (let r = m.row; r < m.row + m.rows && r < model.rows; r++) out[r] = v;
  }
  return out;
}

/** 칸 값의 표시 글자(그 칸의 표시 형식대로). */
export function cellText(model: SheetModel, v: CellValue | undefined, row: number, col: number): string {
  const s = model.styles[model.cells[row * model.cols + col]?.s ?? 0];
  return formatValue(v, s?.numberFormat ?? "");
}

/** 조회조건 항목 '빈칸'의 값: 빈 칸의 표시 글자(`""`)와 같아서 고르면 빈 칸인 행만 남는다. */
export const BLANK = "";
/** 조회조건 항목 '빈칸'을 보일 때의 글자. */
export const BLANK_LABEL = "(빈칸)";

/** 조회조건 항목을 보일 글자(빈칸이면 '(빈칸)'). */
export function optionLabel(value: string): string {
  return value === BLANK ? BLANK_LABEL : value;
}

/**
 * 조회조건 드롭다운 항목: 보이는 데이터 행의 값(처음 나온 순서). 빈 칸인 행이 있으면 맨 끝에 [`BLANK`]('빈칸')를 더한다
 * (Excel 필터의 '(필드 값 없음)'처럼).
 */
export function filterOptions(model: SheetModel, col: number): string[] {
  const values = effectiveColumn(model, col);
  const hidden = new Set(model.hiddenRows);
  const seen = new Set<string>();
  let blank = false;
  for (let r = model.headerRows; r < model.rows; r++) {
    if (hidden.has(r)) continue;
    const t = cellText(model, values[r], r, col);
    if (t === BLANK) blank = true;
    else seen.add(t);
  }
  return blank ? [...seen, BLANK] : [...seen];
}

/** 조회조건(열 → 고른 값, [`BLANK`]면 빈 칸)에 맞지 않는 데이터 행. */
export function rowsNotMatching(model: SheetModel, values: Record<number, string>): Set<number> {
  const out = new Set<number>();
  const cols = Object.entries(values).map(([c, want]) => ({
    col: Number(c),
    want,
    values: effectiveColumn(model, Number(c)),
  }));
  if (cols.length === 0) return out;
  for (let r = model.headerRows; r < model.rows; r++) {
    if (!cols.every((c) => cellText(model, c.values[r], r, c.col) === c.want)) out.add(r);
  }
  return out;
}

/** 걸러진 행을 높이 0으로 만든 보기용 시트(값·셀 배열은 원본과 같은 것을 쓴다). */
export function withHiddenRows(model: SheetModel, hide: Set<number>): SheetModel {
  if (hide.size === 0) return model;
  const rowHeights = model.rowHeights.map((h, r) => (hide.has(r) ? 0 : h));
  const hiddenRows = [...new Set([...model.hiddenRows, ...hide])].sort((a, b) => a - b);
  return { ...model, rowHeights, hiddenRows };
}

/**
 * 걸러진 행(`hide`) 때문에 일부 행만 보이는 병합 영역(왼쪽 위 칸 인덱스 → 영역). 그리드는 보이는 행만으로 병합 칸을
 * 그리지만, 가려진 행까지 걸친 칸이라 보는 쪽에서 읽기 전용으로 다룬다. 원래 숨긴 행(`model.hiddenRows`)만 빠진 병합은 넣지 않는다.
 */
export function partlyShownMerges(model: SheetModel, hide: Set<number>): Map<number, CellRange> {
  const out = new Map<number, CellRange>();
  if (hide.size === 0) return out;
  const hiddenBefore = new Set(model.hiddenRows);
  for (const m of model.merges) {
    let filtered = false;
    let shown = false;
    for (let r = m.row; r < Math.min(m.row + m.rows, model.rows); r++) {
      if (hide.has(r)) filtered = true;
      else if (!hiddenBefore.has(r)) shown = true;
    }
    if (filtered && shown) out.set(m.row * model.cols + m.col, m);
  }
  return out;
}

/** 병합 영역의 값이 든 칸(인덱스): 왼쪽 위 칸에 값이 있으면 그 칸, 비었으면 영역 안에서 처음 값이 있는 칸, 모두 비면 왼쪽 위 칸. */
export function mergeValueCell(model: SheetModel, m: CellRange): number {
  const blank = (v: CellValue | undefined) => v === undefined || v === null || v === "";
  const top = m.row * model.cols + m.col;
  if (!blank(model.cells[top]?.v)) return top;
  for (let r = m.row; r < Math.min(m.row + m.rows, model.rows); r++)
    for (let c = m.col; c < Math.min(m.col + m.cols, model.cols); c++) {
      const i = r * model.cols + c;
      if (!blank(model.cells[i]?.v)) return i;
    }
  return top;
}

/** 줄바꿈 칸이 늘릴 수 있는 최대 줄 수(더 긴 내용은 셀 상세에서 본다). */
export const FIT_MAX_LINES = 8;

/**
 * 줄바꿈(Alt+Enter)이 든 칸이 다 보이도록 행 높이를 늘린 보기용 시트(Excel이 줄바꿈을 넣으면 행 높이를 맞추는 것처럼).
 * 늘리기만 하고 줄이지 않는다. 숨긴 행(높이 0)·여러 행에 걸친 병합은 그대로 둔다. 바꿀 것이 없으면 `model`을 그대로 돌려준다.
 */
export function fitRowHeights(model: SheetModel, maxLines = FIT_MAX_LINES): SheetModel {
  const tall = new Set<number>();
  for (const m of model.merges) {
    if (m.rows <= 1) continue;
    for (let r = m.row; r < m.row + m.rows; r++)
      for (let c = m.col; c < m.col + m.cols; c++) tall.add(r * model.cols + c);
  }
  let heights: number[] | null = null;
  for (let r = 0; r < model.rows; r++) {
    const now = model.rowHeights[r] ?? 0;
    if (now <= 0) continue;
    let need = now;
    for (let c = 0; c < model.cols; c++) {
      const i = r * model.cols + c;
      const v = model.cells[i]?.v;
      if (typeof v !== "string" || !v.includes("\n") || tall.has(i)) continue;
      const lines = Math.min(maxLines, v.split("\n").length);
      const size = model.styles[model.cells[i]?.s ?? 0]?.fontSize || 11;
      // 줄 높이 1.25 + 위아래 여백(pt)
      need = Math.max(need, Math.ceil(lines * size * 1.25 + 4));
    }
    if (need > now) {
      heights ??= [...model.rowHeights];
      heights[r] = need;
    }
  }
  return heights ? { ...model, rowHeights: heights } : model;
}

/**
 * 행 `from`부터 `count`개를 지울 때 첫 행만 잘리는 병합(여러 행에 걸친 병합의 첫 행이 범위 안이고 아래로 범위를 넘는 것 —
 * 지우면 병합의 값이 사라진다). Rust `collect_core::structure::cut_merge`와 같다.
 */
export function cutMerge(model: SheetModel, from: number, count: number): CellRange | null {
  const end = from + count;
  return (
    model.merges.find((m) => (m.rows > 1 || m.cols > 1) && from <= m.row && m.row < end && m.row + m.rows - 1 >= end) ??
    null
  );
}
