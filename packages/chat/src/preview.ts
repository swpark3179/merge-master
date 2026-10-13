import type { CellRange, CellRef } from "@mm/shared";
import { colLetters, formatRange } from "@mm/shared";

/** 셀 참조 미리보기 카드에 그릴 것(시트·주소·크기와 처음 몇 칸의 값). */
export interface RefPreview {
  sheet: string;
  /** 시트 주소(`F6:H8`). */
  addr: string;
  /** `3행 × 4열 · 12칸` */
  size: string;
  /** 열 글자와 그 열의 제목(라벨). 값을 모르면 비어 있다. */
  heads: { letter: string; label: string }[];
  rows: { n: number; cells: PreviewCell[] }[];
  /** 다 못 보인 행·열(`아래로 5행 더 · 오른쪽으로 1열 더`). */
  more?: string;
}

export interface PreviewCell {
  text: string;
  numeric: boolean;
  /** 이 화면에서 볼 수 없는 칸(남의 행). */
  hidden: boolean;
}

/** 미리보기의 원본: 그 시트의 모델 크기·원점과 칸 표시 글자. `text`가 없으면 값 없이 이름·주소·크기만. */
export interface PreviewSource {
  sheet: string;
  origin: CellRef;
  rows: number;
  cols: number;
  /** 칸(모델 좌표) 표시 글자와 숫자인지. */
  text?: (row: number, col: number) => { text: string; numeric: boolean };
  /** 열(모델 좌표)의 제목. */
  label?: (col: number) => string | undefined;
  /** 이 화면에서 볼 수 없는 행(모델 좌표). */
  hidden?: (row: number) => boolean;
}

const MAX_ROWS = 3;
const MAX_COLS = 4;

/** `range`는 그 시트의 모델 좌표. 시트 밖으로 나간 부분은 크기에만 센다. */
export function buildRefPreview(src: PreviewSource, range: CellRange): RefPreview {
  const sheetRange = { ...range, row: src.origin.row + range.row, col: src.origin.col + range.col };
  const out: RefPreview = {
    sheet: src.sheet,
    addr: formatRange(sheetRange),
    size: `${range.rows}행 × ${range.cols}열 · ${(range.rows * range.cols).toLocaleString()}칸`,
    heads: [],
    rows: [],
  };
  const r0 = Math.max(0, range.row);
  const c0 = Math.max(0, range.col);
  const r1 = Math.min(src.rows, range.row + range.rows, r0 + MAX_ROWS);
  const c1 = Math.min(src.cols, range.col + range.cols, c0 + MAX_COLS);
  if (!src.text || r1 <= r0 || c1 <= c0) return out;
  for (let c = c0; c < c1; c++) out.heads.push({ letter: colLetters(src.origin.col + c), label: src.label?.(c) ?? "" });
  for (let r = r0; r < r1; r++) {
    const hidden = src.hidden?.(r) ?? false;
    const cells: PreviewCell[] = [];
    for (let c = c0; c < c1; c++) {
      const t = hidden ? { text: "", numeric: false } : src.text(r, c);
      cells.push({ ...t, hidden });
    }
    out.rows.push({ n: src.origin.row + r + 1, cells });
  }
  const moreRows = range.row + range.rows - r1;
  const moreCols = range.col + range.cols - c1;
  const more = [
    moreRows > 0 && `아래로 ${moreRows.toLocaleString()}행 더`,
    moreCols > 0 && `오른쪽으로 ${moreCols}열 더`,
  ]
    .filter(Boolean)
    .join(" · ");
  if (more) out.more = more;
  return out;
}
