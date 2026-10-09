import { cellText, effectiveColumn } from "@mm/grid";
import type { SheetModel } from "@mm/shared";
import { FLAG } from "@mm/shared";

export { effectiveColumn, filterOptions, withHiddenRows } from "@mm/grid";

/**
 * 1f 조회조건·'내 담당만'·'필수 미입력만' 필터. 걸러진 데이터 행은 높이 0(숨김)으로 그린다.
 * 병합 영역은 그리드가 보이는 행만으로 다시 그리므로(높이 합) 따로 쪼개지 않아도 된다.
 */
export interface RowFilter {
  /** 열 → 고른 값(표시 글자). 없으면 전체. */
  values: Record<number, string>;
  mineOnly: boolean;
  missingOnly: boolean;
}

export const NO_FILTER: RowFilter = { values: {}, mineOnly: false, missingOnly: false };

export function isFiltering(f: RowFilter): boolean {
  return f.mineOnly || f.missingOnly || Object.keys(f.values).length > 0;
}

/** 행마다 '내 행'인지(행 담당자이거나 내가 입력할 칸이 있는 행). */
export function myRowsOf(model: SheetModel, flags: Uint8Array, rowOwner: number[], me: number): Uint8Array {
  const out = new Uint8Array(model.rows);
  for (let r = 0; r < model.rows; r++) {
    if (rowOwner[r] === me) out[r] = 1;
    else for (let c = 0; c < model.cols; c++) if (flags[r * model.cols + c]! & FLAG.MINE) out[r] = 1;
  }
  return out;
}

/** 내 필수 칸 중 빈 칸 수(행마다). */
export function missingOf(model: SheetModel, flags: Uint8Array, row: number): number {
  let n = 0;
  for (let c = 0; c < model.cols; c++) {
    const i = row * model.cols + c;
    const f = flags[i]!;
    if (f & FLAG.MINE && f & FLAG.REQUIRED) {
      const v = model.cells[i]?.v;
      if (v === undefined || v === null || v === "") n++;
    }
  }
  return n;
}

/** 필터에 걸려 숨길 데이터 행. 머리글 행은 언제나 보인다. */
export function filteredOut(model: SheetModel, flags: Uint8Array, myRows: Uint8Array, f: RowFilter): Set<number> {
  const out = new Set<number>();
  if (!isFiltering(f)) return out;
  const cols = Object.entries(f.values).map(([c, v]) => ({
    col: Number(c),
    want: v,
    values: effectiveColumn(model, Number(c)),
  }));
  for (let r = model.headerRows; r < model.rows; r++) {
    const keep =
      (!f.mineOnly || myRows[r] === 1) &&
      (!f.missingOnly || missingOf(model, flags, r) > 0) &&
      cols.every((c) => cellText(model, c.values[r], r, c.col) === c.want);
    if (!keep) out.add(r);
  }
  return out;
}
