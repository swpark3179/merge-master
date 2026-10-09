import type { CellRange } from "./generated";

import { colLetters } from "./format";

const REF = /^\$?([A-Za-z]{1,3})\$?(\d{1,7})$/;

function colIndex(letters: string): number {
  let n = 0;
  for (const ch of letters.toUpperCase()) n = n * 26 + (ch.charCodeAt(0) - 64);
  return n - 1;
}

/** `"A4:G25"`, `"$A$4:$G$25"`, `"F6"` → 0부터 센 범위. 해석할 수 없으면 `null`. Rust `CellRange::parse`와 같은 규칙. */
export function parseRange(text: string): CellRange | null {
  const parts = text.trim().split(":");
  if (parts.length < 1 || parts.length > 2) return null;
  const refs = parts.map((p) => REF.exec(p.trim()));
  if (refs.some((m) => !m)) return null;
  const [a, b = a] = refs.map((m) => ({ row: Number(m![2]) - 1, col: colIndex(m![1]!) }));
  if (!a || !b || a.row < 0 || b.row < 0) return null;
  const row = Math.min(a.row, b.row);
  const col = Math.min(a.col, b.col);
  return { row, col, rows: Math.abs(a.row - b.row) + 1, cols: Math.abs(a.col - b.col) + 1 };
}

/** 범위 → `"A4:G25"` (한 칸이면 `"F6"`). */
export function formatRange(r: CellRange): string {
  const tl = `${colLetters(r.col)}${r.row + 1}`;
  if (r.rows === 1 && r.cols === 1) return tl;
  return `${tl}:${colLetters(r.col + r.cols - 1)}${r.row + r.rows}`;
}

/** 열 범위 표시: `colSpan(0, 4)` → `"A–D열"`, 한 열이면 `"A열"`. */
export function colSpanLabel(firstCol: number, count: number): string {
  if (count <= 0) return "없음";
  if (count === 1) return `${colLetters(firstCol)}열`;
  return `${colLetters(firstCol)}–${colLetters(firstCol + count - 1)}열`;
}

export type TextPart = { text: string } | { ref: string; range: CellRange };

/** 채팅 글의 `#F7`, `#F7:H7` 셀 참조를 나눈다(시트 주소). 누르면 그 칸으로 간다. */
export function splitCellRefs(text: string): TextPart[] {
  const out: TextPart[] = [];
  const re = /#([A-Za-z]{1,3}\d{1,7}(?::[A-Za-z]{1,3}\d{1,7})?)/g;
  let last = 0;
  for (let m = re.exec(text); m; m = re.exec(text)) {
    const range = parseRange(m[1]!);
    if (!range) continue;
    if (m.index > last) out.push({ text: text.slice(last, m.index) });
    out.push({ ref: m[1]!.toUpperCase(), range });
    last = m.index + m[0].length;
  }
  if (last < text.length) out.push({ text: text.slice(last) });
  return out;
}
