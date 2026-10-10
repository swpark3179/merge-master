import type { CellRange, ColumnRole, ConfigView } from "@mm/shared";
import { FLAG } from "@mm/shared";

/** 고른 범위를 누가 편집할 수 있는지(그리드 아래 줄). 판정 비트는 Rust 권한 엔진이 계산한 `flags`를 그대로 쓴다. */
export interface RangeAccess {
  /** 센 칸 수(병합에 덮인 칸·숨긴 칸 제외). */
  cells: number;
  /** 입력할 수 없는 칸(잠금·수식·머리글). */
  locked: number;
  /** 담당자가 없는 입력 칸(누구나). */
  open: number;
  /** 입력 칸을 맡은 사람(명단 인덱스, 명단 순). */
  people: number[];
  required: number;
}

/** 너무 큰 범위는 앞부분만 센다(화면 표시용). */
const MAX_CELLS = 200_000;

export function rangeAccess(view: ConfigView, flags: Uint8Array, cols: number, range: CellRange): RangeAccess {
  const out: RangeAccess = { cells: 0, locked: 0, open: 0, people: [], required: 0 };
  const owners = new Set<number>();
  const ranges = view.config.assignment.ranges
    .map((a) => ({ r: a.range, p: view.roster.indexOf(a.person.trim()) }))
    .filter((a) => a.p >= 0);
  for (let r = range.row; r < range.row + range.rows && out.cells < MAX_CELLS; r++) {
    for (let c = range.col; c < range.col + range.cols; c++) {
      const f = flags[r * cols + c] ?? 0;
      if (f & (FLAG.COVERED | FLAG.HIDDEN)) continue;
      out.cells++;
      if (!(f & FLAG.INPUT)) {
        out.locked++;
        continue;
      }
      if (f & FLAG.REQUIRED) out.required++;
      let owned = false;
      const row = view.rowOwner[r] ?? -1;
      if (row >= 0) {
        owners.add(row);
        owned = true;
      }
      for (const a of ranges) {
        if (r >= a.r.row && r < a.r.row + a.r.rows && c >= a.r.col && c < a.r.col + a.r.cols) {
          owners.add(a.p);
          owned = true;
        }
      }
      if (!owned) out.open++;
    }
  }
  out.people = [...owners].sort((a, b) => a - b);
  return out;
}

/** 일부 칸만 고른 범위의 실제 역할(셀 예외 반영): 모두 입력이면 입력, 모두 잠금이면 잠금, 섞였으면 `null`. 머리글·병합에
 *  덮인 칸·숨긴 칸은 세지 않는다(셀 수 있는 칸이 없으면 `null`). */
export function cellsRole(flags: Uint8Array, cols: number, range: CellRange): ColumnRole | null {
  let input = 0;
  let locked = 0;
  let seen = 0;
  for (let r = range.row; r < range.row + range.rows && seen < MAX_CELLS; r++) {
    for (let c = range.col; c < range.col + range.cols; c++) {
      const f = flags[r * cols + c] ?? 0;
      if (f & (FLAG.HEADER | FLAG.COVERED | FLAG.HIDDEN)) continue;
      seen++;
      if (f & FLAG.INPUT) input++;
      else locked++;
      if (input > 0 && locked > 0) return null;
    }
  }
  return input > 0 ? "input" : locked > 0 ? "locked" : null;
}
