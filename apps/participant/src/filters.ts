import { cellText, effectiveColumn } from "@mm/grid";
import type { SheetModel } from "@mm/shared";
import { FLAG } from "@mm/shared";

export { effectiveColumn, filterOptions, withHiddenRows } from "@mm/grid";

/**
 * 1f 조회조건·'내 담당만'·'필수 미입력만' 필터. 걸러진 데이터 행은 높이 0(숨김)으로 그린다.
 * 병합 영역은 그리드가 보이는 행만으로 다시 그리므로(높이 합) 따로 쪼개지 않아도 된다.
 */
export interface RowFilter {
  /** 열 → 고른 값들(표시 글자, 그중 하나면 보인다). 없거나 비면 전체. */
  values: Record<number, readonly string[]>;
  mineOnly: boolean;
  missingOnly: boolean;
}

export const NO_FILTER: RowFilter = { values: {}, mineOnly: false, missingOnly: false };

export function isFiltering(f: RowFilter): boolean {
  return f.mineOnly || f.missingOnly || Object.values(f.values).some((v) => v.length > 0);
}

/** 행마다 '내 행'인지(행 담당자이거나 내가 입력할 칸이 있는 행). */
export function myRowsOf(
  model: SheetModel,
  flags: Uint8Array,
  rowOwner: number[],
  me: number,
  editableAll = false,
): Uint8Array {
  const out = new Uint8Array(model.rows);
  for (let r = 0; r < model.rows; r++) {
    if (rowOwner[r] === me) out[r] = 1;
    // '누구나 편집' 모드에서는 입력 가능 칸이 곧 내 칸이 아니므로 담당자 열만 본다.
    else if (!editableAll) for (let c = 0; c < model.cols; c++) if (flags[r * model.cols + c]! & FLAG.MINE) out[r] = 1;
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
  const cols = Object.entries(f.values)
    .filter(([, v]) => v.length > 0)
    .map(([c, v]) => ({ col: Number(c), want: new Set(v), values: effectiveColumn(model, Number(c)) }));
  for (let r = model.headerRows; r < model.rows; r++) {
    const keep =
      (!f.mineOnly || myRows[r] === 1) &&
      (!f.missingOnly || missingOf(model, flags, r) > 0) &&
      cols.every((c) => c.want.has(cellText(model, c.values[r], r, c.col)));
    if (!keep) out.add(r);
  }
  return out;
}

/** 조회조건 항목 검색: 대소문자·앞뒤 공백을 가리지 않는 부분 일치(나온 순서 그대로). 빈 검색어면 모두. */
export function matchOptions(options: readonly string[], query: string): string[] {
  const q = query.trim().toLocaleLowerCase();
  return q ? options.filter((o) => o.toLocaleLowerCase().includes(q)) : [...options];
}

/** 조회조건 값 하나를 고르거나 뺀다(고른 순서 그대로). */
export function toggleValue(value: readonly string[], v: string): string[] {
  return value.includes(v) ? value.filter((x) => x !== v) : [...value, v];
}

/** 검색 결과를 모두 고른다. 이미 모두 골랐으면 검색 결과만 뺀다(다른 고른 값은 그대로). */
export function toggleHits(value: readonly string[], hits: readonly string[]): string[] {
  if (hits.every((h) => value.includes(h))) return value.filter((x) => !hits.includes(x));
  return [...value, ...hits.filter((h) => !value.includes(h))];
}

/** 조회조건 단추 글자: 없으면 '전체', 하나면 그 값, 여럿이면 '재무팀 외 2'. */
export function selectionLabel(value: readonly string[]): string {
  if (value.length === 0) return "전체";
  return value.length === 1 ? value[0]! : `${value[0]} 외 ${value.length - 1}`;
}
