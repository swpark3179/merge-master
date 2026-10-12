import type { CellRange, SheetModel } from "@mm/shared";
import { PT_TO_PX } from "@mm/shared";

/** 열 문자 머리글 높이와 행 번호 칸 너비(px). */
export const HEADER_H = 22;
export const GUTTER_W = 40;

/**
 * 그리드 배치 정보. 좌표는 "데이터 영역" 기준 px이며, 화면에 그릴 때 머리글(HEADER_H)·행 번호(GUTTER_W)를 더한다.
 */
export interface GridLayout {
  rows: number;
  cols: number;
  /** 길이 cols+1, colX[c] = c열 왼쪽 끝. */
  colX: Float64Array;
  /** 길이 rows+1, rowY[r] = r행 위쪽 끝. */
  rowY: Float64Array;
  frozenRows: number;
  frozenCols: number;
  frozenW: number;
  frozenH: number;
  /** 셀마다 병합 영역 인덱스, 병합 아님 = -1. */
  mergeAt: Int32Array;
  merges: CellRange[];
}

function prefix(sizesPt: number[], count: number, fallbackPt: number): Float64Array {
  const out = new Float64Array(count + 1);
  for (let i = 0; i < count; i++) {
    const pt = sizesPt[i] ?? fallbackPt;
    out[i + 1] = out[i]! + Math.round(Math.max(pt, 0) * PT_TO_PX);
  }
  return out;
}

export function buildLayout(model: SheetModel): GridLayout {
  const { rows, cols } = model;
  const colX = prefix(model.colWidths, cols, 48);
  const rowY = prefix(model.rowHeights, rows, 15);
  // 머리글 행은 틀 고정이 없더라도 항상 위에 고정한다.
  const frozenRows = Math.min(rows, Math.max(model.frozenRows, model.headerRows));
  const frozenCols = Math.min(cols, model.frozenCols);

  const mergeAt = new Int32Array(rows * cols).fill(-1);
  model.merges.forEach((m, i) => {
    for (let r = m.row; r < m.row + m.rows && r < rows; r++) {
      for (let c = m.col; c < m.col + m.cols && c < cols; c++) mergeAt[r * cols + c] = i;
    }
  });

  return {
    rows,
    cols,
    colX,
    rowY,
    frozenRows,
    frozenCols,
    frozenW: colX[frozenCols]!,
    frozenH: rowY[frozenRows]!,
    mergeAt,
    merges: model.merges,
  };
}

/** `offsets[i] <= pos`인 가장 큰 i (0 ~ offsets.length-2로 제한). */
export function indexAt(offsets: Float64Array, pos: number): number {
  let lo = 0;
  let hi = offsets.length - 2;
  if (hi < 0) return 0;
  while (lo < hi) {
    const mid = (lo + hi + 1) >> 1;
    if (offsets[mid]! <= pos) lo = mid;
    else hi = mid - 1;
  }
  return lo;
}

export interface Span {
  first: number;
  last: number;
}

/**
 * 스크롤 위치에서 실제로 그릴 행(또는 열) 범위. 고정 영역 뒤부터 시작하고, 위아래로 `overscan`만큼 더 그린다.
 * 그릴 것이 없으면 `first > last`.
 */
export function visibleSpan(
  offsets: Float64Array,
  frozenCount: number,
  frozenSize: number,
  scroll: number,
  viewport: number,
  overscan: number,
): Span {
  const count = offsets.length - 1;
  if (frozenCount >= count) return { first: count, last: count - 1 };
  const start = scroll + frozenSize;
  const end = scroll + Math.max(viewport, 0);
  const first = Math.max(frozenCount, indexAt(offsets, start) - overscan);
  const last = Math.min(count - 1, indexAt(offsets, end) + overscan);
  return { first, last };
}

/** 크기 0인 행/열(숨김). */
export function isHidden(offsets: Float64Array, i: number): boolean {
  return offsets[i + 1] === offsets[i];
}

/**
 * `from`에서 `step`(±1)만큼 움직이되 숨긴 행/열은 건너뛴다. 그 방향에 보이는 것이 없으면 `from`을 돌려준다.
 */
export function nextVisible(offsets: Float64Array, from: number, step: number): number {
  const count = offsets.length - 1;
  for (let i = from + step; i >= 0 && i < count; i += step) {
    if (!isHidden(offsets, i)) return i;
  }
  return from;
}

/** 셀 (r, c)가 병합 영역이면 그 영역, 아니면 1×1 범위. */
export function cellBox(layout: GridLayout, r: number, c: number): CellRange {
  const m = layout.mergeAt[r * layout.cols + c]!;
  return m >= 0 ? layout.merges[m]! : { row: r, col: c, rows: 1, cols: 1 };
}

/** 화면(콘텐츠) 좌표의 사각형. */
export function boxRect(layout: GridLayout, box: CellRange) {
  const x = layout.colX[box.col]!;
  const y = layout.rowY[box.row]!;
  return {
    left: GUTTER_W + x,
    top: HEADER_H + y,
    width: layout.colX[box.col + box.cols]! - x,
    height: layout.rowY[box.row + box.rows]! - y,
  };
}

/** 고정 영역으로 나뉜 네 겹: 모서리(행·열 고정)·위(행만 고정)·왼쪽(열만 고정)·본문(스크롤). */
export type Layer = "corner" | "top" | "left" | "body";

/** 칸(병합이면 왼쪽 위 칸)이 그려지는 겹. 겹마다 따로 고정되므로 그 칸 위에 겹쳐 그릴 것도 같은 겹에 둔다. */
export function layerOf(layout: GridLayout, row: number, col: number): Layer {
  const top = row < layout.frozenRows;
  const left = col < layout.frozenCols;
  return top ? (left ? "corner" : "top") : left ? "left" : "body";
}

/** 범위를 고정 행·열 경계에서 겹마다의 조각(최대 4개)으로 나눈다. 빈 조각은 뺀다. */
export function splitByFrozen(layout: GridLayout, range: CellRange): { layer: Layer; range: CellRange }[] {
  const fr = layout.frozenRows;
  const fc = layout.frozenCols;
  const end = { row: range.row + range.rows, col: range.col + range.cols };
  const rows = [
    [range.row, Math.min(end.row, fr)],
    [Math.max(range.row, fr), end.row],
  ] as const;
  const cols = [
    [range.col, Math.min(end.col, fc)],
    [Math.max(range.col, fc), end.col],
  ] as const;
  const out: { layer: Layer; range: CellRange }[] = [];
  for (const [i, [r0, r1]] of rows.entries())
    for (const [j, [c0, c1]] of cols.entries()) {
      if (r1 <= r0 || c1 <= c0) continue;
      const layer: Layer = i === 0 ? (j === 0 ? "corner" : "top") : j === 0 ? "left" : "body";
      out.push({ layer, range: { row: r0, col: c0, rows: r1 - r0, cols: c1 - c0 } });
    }
  return out;
}
