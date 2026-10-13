import type { CellRange, CellRef, SheetModel } from "@mm/shared";

import type { GridLayout } from "./layout";

/** 범위 선택: 처음 누른 칸(anchor, 활성 칸)과 끝 칸(focus). */
export interface GridSelection {
  anchor: CellRef;
  focus: CellRef;
}

export function cellSelection(at: CellRef): GridSelection {
  return { anchor: at, focus: at };
}

/** 열 c0~c1의 데이터 행 전체(머리글 아래). 데이터 행이 없으면 모든 행. */
export function columnSelection(layout: GridLayout, headerRows: number, c0: number, c1: number): GridSelection {
  const top = headerRows < layout.rows ? headerRows : 0;
  return { anchor: { row: top, col: c0 }, focus: { row: layout.rows - 1, col: c1 } };
}

export function rowSelection(layout: GridLayout, r0: number, r1: number): GridSelection {
  return { anchor: { row: r0, col: 0 }, focus: { row: r1, col: layout.cols - 1 } };
}

export function intersects(a: CellRange, b: CellRange): boolean {
  return a.row < b.row + b.rows && b.row < a.row + a.rows && a.col < b.col + b.cols && b.col < a.col + a.cols;
}

export function containsCell(r: CellRange, row: number, col: number): boolean {
  return row >= r.row && row < r.row + r.rows && col >= r.col && col < r.col + r.cols;
}

/** 걸친 병합 영역을 모두 품도록 범위를 넓힌다(Excel과 같은 동작). */
export function expandToMerges(layout: GridLayout, range: CellRange): CellRange {
  let r = range;
  for (let changed = true; changed;) {
    changed = false;
    for (const m of layout.merges) {
      if (!intersects(r, m)) continue;
      const row = Math.min(r.row, m.row);
      const col = Math.min(r.col, m.col);
      const lastRow = Math.max(r.row + r.rows, m.row + m.rows);
      const lastCol = Math.max(r.col + r.cols, m.col + m.cols);
      if (row !== r.row || col !== r.col || lastRow !== r.row + r.rows || lastCol !== r.col + r.cols) {
        r = { row, col, rows: lastRow - row, cols: lastCol - col };
        changed = true;
      }
    }
  }
  return r;
}

/** 선택 → 사각형 범위(병합 포함). */
export function selectionRange(layout: GridLayout, sel: GridSelection): CellRange {
  const row = Math.min(sel.anchor.row, sel.focus.row);
  const col = Math.min(sel.anchor.col, sel.focus.col);
  const range = {
    row,
    col,
    rows: Math.abs(sel.anchor.row - sel.focus.row) + 1,
    cols: Math.abs(sel.anchor.col - sel.focus.col) + 1,
  };
  return expandToMerges(layout, range);
}

/** 범위 안 실제 칸 수(병합 영역은 한 칸으로 센다). */
export function countCells(layout: GridLayout, range: CellRange): number {
  let n = 0;
  for (let r = range.row; r < range.row + range.rows; r++) {
    for (let c = range.col; c < range.col + range.cols; c++) {
      const m = layout.mergeAt[r * layout.cols + c]!;
      if (m < 0) n++;
      else {
        const mr = layout.merges[m]!;
        if (mr.row === r && mr.col === c) n++;
      }
    }
  }
  return n;
}

/** 머리글을 누른 것과 같은 선택: 열은 데이터 행 전체(데이터 행이 없으면 모든 행), 행은 모든 열. */
export function lineSelection(
  model: Pick<SheetModel, "rows" | "cols" | "headerRows">,
  axis: "col" | "row",
  from: number,
  to: number = from,
): GridSelection {
  if (axis === "col") {
    const top = model.headerRows < model.rows ? model.headerRows : 0;
    return { anchor: { row: top, col: from }, focus: { row: model.rows - 1, col: to } };
  }
  return { anchor: { row: from, col: 0 }, focus: { row: to, col: model.cols - 1 } };
}

/** 오른쪽 클릭한 곳: 칸, 열 머리글, 행 머리글(그리드의 `GridHit`과 같은 모양). */
export type LineHit =
  { kind: "cell"; row: number; col: number } | { kind: "col"; col: number } | { kind: "row"; row: number };

/**
 * 오른쪽 클릭 메뉴가 다룰 선택(Excel처럼): 누른 곳이 지금 선택 안이면 그대로, 밖이면 그 칸·열·행 하나를 고른다.
 * 선택 범위는 병합을 넓히지 않은 사각형(`anchor`~`focus`)으로 본다.
 */
export function selectForMenu(
  model: Pick<SheetModel, "rows" | "cols" | "headerRows">,
  sel: GridSelection | null,
  hit: LineHit,
): GridSelection {
  const r = sel && {
    row: Math.min(sel.anchor.row, sel.focus.row),
    col: Math.min(sel.anchor.col, sel.focus.col),
    rows: Math.abs(sel.anchor.row - sel.focus.row) + 1,
    cols: Math.abs(sel.anchor.col - sel.focus.col) + 1,
  };
  if (hit.kind === "cell")
    return r && containsCell(r, hit.row, hit.col) ? sel! : cellSelection({ row: hit.row, col: hit.col });
  if (hit.kind === "col") {
    const whole = r && r.row <= model.headerRows && r.row + r.rows >= model.rows;
    return whole && hit.col >= r.col && hit.col < r.col + r.cols ? sel! : lineSelection(model, "col", hit.col);
  }
  const whole = r && r.col === 0 && r.cols === model.cols;
  return whole && hit.row >= r.row && hit.row < r.row + r.rows ? sel! : lineSelection(model, "row", hit.row);
}

/** 앞뒤의 숨은 행을 잘라 낸 범위(가운데 숨은 행은 그대로 둔다). 모두 숨었으면 `null`. */
export function trimHiddenRows(range: CellRange, hidden: (row: number) => boolean): CellRange | null {
  let first = range.row;
  let last = range.row + range.rows - 1;
  while (first <= last && hidden(first)) first++;
  while (last >= first && hidden(last)) last--;
  if (first > last) return null;
  return { ...range, row: first, rows: last - first + 1 };
}
