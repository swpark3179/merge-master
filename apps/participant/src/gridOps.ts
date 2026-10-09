import type { CellRange, ColumnRule, SheetModel } from "@mm/shared";
import { FLAG, normalize } from "@mm/shared";

import type { CellChange } from "./edits";

/** 범위 지우기·붙여넣기·복사에 필요한 것(화면 기준: 숨긴 행·열은 건너뛴다). */
export interface EditContext {
  model: SheetModel;
  flags: Uint8Array;
  columns: ColumnRule[];
  /** 다른 사람이 편집 중인 칸(셀 인덱스). */
  locks: Map<number, string>;
  /** 화면에서 숨긴 행(필터·남의 행 숨김·원본 숨김)과 열. */
  hiddenRows: Set<number>;
  hiddenCols: Set<number>;
  readOnly: boolean;
}

export function canEdit(ctx: EditContext, row: number, col: number): boolean {
  const i = row * ctx.model.cols + col;
  return !ctx.readOnly && ((ctx.flags[i] ?? 0) & FLAG.MINE) !== 0 && !ctx.locks.has(i);
}

function visible(ctx: EditContext, row: number, col: number): boolean {
  return !ctx.hiddenRows.has(row) && !ctx.hiddenCols.has(col);
}

export interface BulkResult {
  changes: CellChange[];
  /** 입력할 수 없는 칸(남의 칸·입력 금지·편집 중). */
  skipped: number;
  /** 형식이 맞지 않아 건너뛴 칸. */
  invalid: number;
  firstError?: string;
}

/** 범위의 내 칸을 비운다(Delete). */
export function clearRange(ctx: EditContext, rect: CellRange): BulkResult {
  const out: BulkResult = { changes: [], skipped: 0, invalid: 0 };
  for (let r = rect.row; r < rect.row + rect.rows; r++) {
    for (let c = rect.col; c < rect.col + rect.cols; c++) {
      if (!visible(ctx, r, c)) continue;
      const f = ctx.flags[r * ctx.model.cols + c] ?? 0;
      if (f & (FLAG.COVERED | FLAG.HEADER)) continue;
      if (!canEdit(ctx, r, c)) {
        if (ctx.model.cells[r * ctx.model.cols + c]?.v != null) out.skipped++;
        continue;
      }
      out.changes.push({ row: r, col: c, value: null });
    }
  }
  return out;
}

function put(ctx: EditContext, out: BulkResult, row: number, col: number, text: string) {
  if (!canEdit(ctx, row, col)) {
    out.skipped++;
    return;
  }
  const n = normalize(ctx.columns[col]?.format ?? { kind: "text" }, text);
  if (!n.ok) {
    out.invalid++;
    out.firstError ??= n.error;
    return;
  }
  out.changes.push({ row, col, value: n.value });
}

/** 보이는 행(열)만 세어 `from`부터 `count`개. */
function visibleRun(from: number, count: number, limit: number, hidden: Set<number>): number[] {
  const out: number[] = [];
  for (let i = from; i < limit && out.length < count; i++) if (!hidden.has(i)) out.push(i);
  return out;
}

/**
 * Excel에서 복사한 표를 붙인다. 한 칸을 범위에 붙이면 범위를 채우고, 그 밖에는 범위 왼쪽 위부터 보이는 행·열에 붙인다.
 */
export function pasteRange(ctx: EditContext, rect: CellRange, clip: string[][]): BulkResult {
  const out: BulkResult = { changes: [], skipped: 0, invalid: 0 };
  if (clip.length === 0) return out;
  const single = clip.length === 1 && clip[0]!.length === 1;
  if (single && (rect.rows > 1 || rect.cols > 1)) {
    for (let r = rect.row; r < rect.row + rect.rows; r++) {
      for (let c = rect.col; c < rect.col + rect.cols; c++) {
        if (!visible(ctx, r, c) || (ctx.flags[r * ctx.model.cols + c] ?? 0) & FLAG.COVERED) continue;
        put(ctx, out, r, c, clip[0]![0]!);
      }
    }
    return out;
  }
  const width = Math.max(...clip.map((r) => r.length));
  const rows = visibleRun(rect.row, clip.length, ctx.model.rows, ctx.hiddenRows);
  const cols = visibleRun(rect.col, width, ctx.model.cols, ctx.hiddenCols);
  rows.forEach((r, i) => {
    cols.forEach((c, j) => {
      const text = clip[i]![j];
      if (text === undefined || (ctx.flags[r * ctx.model.cols + c] ?? 0) & FLAG.COVERED) return;
      put(ctx, out, r, c, text);
    });
  });
  return out;
}

/** 범위를 표로(보이는 행·열만). `text`는 칸의 표시 글자. */
export function copyRange(ctx: EditContext, rect: CellRange, text: (row: number, col: number) => string): string[][] {
  const rows: string[][] = [];
  for (let r = rect.row; r < rect.row + rect.rows; r++) {
    if (ctx.hiddenRows.has(r)) continue;
    const row: string[] = [];
    for (let c = rect.col; c < rect.col + rect.cols; c++) if (!ctx.hiddenCols.has(c)) row.push(text(r, c));
    rows.push(row);
  }
  return rows;
}
