import type { CellRange, ConfigView } from "@mm/shared";
import { FLAG } from "@mm/shared";

/** 고른 범위를 누가 편집할 수 있는지(그리드 아래 줄). 판정 비트는 Rust 권한 엔진이 계산한 `flags`를 그대로 쓴다. */
export interface RangeAccess {
  /** 센 칸 수(병합에 덮인 칸·숨긴 칸 제외). */
  cells: number;
  /** 입력할 수 없는 칸(잠금·머리글, '수식 칸'을 잠그면 수식). */
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
