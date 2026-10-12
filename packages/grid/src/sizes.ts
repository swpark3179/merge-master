import type { Axis, CellRange, SheetModel } from "@mm/shared";

/**
 * 머리글 경계를 끌어 바꿀 줄: 머리글로 여러 열(행)을 골라 두고 그중 하나의 경계를 끌면 고른 열(행) 모두(Excel처럼),
 * 아니면 끈 줄 하나. 열은 데이터 행 전체(머리글 포함)를, 행은 모든 열을 골랐을 때만 함께 바꾼다.
 */
export function resizeSpan(
  model: Pick<SheetModel, "rows" | "cols" | "headerRows">,
  range: CellRange | null,
  axis: Axis,
  index: number,
): { from: number; count: number } {
  if (
    range &&
    axis === "col" &&
    range.row <= model.headerRows &&
    range.row + range.rows >= model.rows &&
    index >= range.col &&
    index < range.col + range.cols
  )
    return { from: range.col, count: range.cols };
  if (
    range &&
    axis === "row" &&
    range.col === 0 &&
    range.cols === model.cols &&
    index >= range.row &&
    index < range.row + range.rows
  )
    return { from: range.row, count: range.rows };
  return { from: index, count: 1 };
}

/**
 * [`resizeSpan`]을 보이는 줄끼리 이어진 구간으로 나눈다. `model`은 화면에 그리는 시트라, 필터·남의 행 숨김으로 숨은
 * 줄(`hiddenRows`·`hiddenCols`)은 건너뛴다(보이지 않는 행의 높이를 몰래 바꾸지 않게). 끈 줄 하나면 그 줄 하나.
 */
export function resizeRuns(
  model: Pick<SheetModel, "rows" | "cols" | "headerRows" | "hiddenRows" | "hiddenCols">,
  range: CellRange | null,
  axis: Axis,
  index: number,
): { from: number; count: number }[] {
  const { from, count } = resizeSpan(model, range, axis, index);
  if (count === 1) return [{ from, count }];
  const hidden = new Set(axis === "col" ? model.hiddenCols : model.hiddenRows);
  const out: { from: number; count: number }[] = [];
  let start = -1;
  for (let i = from; i <= from + count; i++) {
    const shown = i < from + count && !hidden.has(i);
    if (shown && start < 0) start = i;
    else if (!shown && start >= 0) {
      out.push({ from: start, count: i - start });
      start = -1;
    }
  }
  return out;
}

/**
 * `from`부터 `count`개 열 너비(행 높이)를 `size`(pt)로 바꾼 새 시트(서버 `resized`·호스트 `resized` 피드). 숨긴 줄
 * (`hiddenCols`·`hiddenRows` — 남의 행 숨김으로 숨은 행 포함)은 그대로 둔다. Rust `collect_core::format::resize`와 같다.
 */
export function withLineSize(model: SheetModel, axis: Axis, from: number, count: number, size: number): SheetModel {
  const col = axis === "col";
  const lines = col ? model.cols : model.rows;
  const sizes = (col ? model.colWidths : model.rowHeights).slice(0, lines);
  while (sizes.length < lines) sizes.push(0);
  const hidden = new Set(col ? model.hiddenCols : model.hiddenRows);
  const end = Math.min(from + Math.max(count, 1), lines);
  for (let i = from; i < end; i++) if (!hidden.has(i)) sizes[i] = size;
  return col ? { ...model, colWidths: sizes } : { ...model, rowHeights: sizes };
}
