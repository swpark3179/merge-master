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

/** Excel 주소 앞에 붙이는 시트 이름: `인력계획!`, 글자·숫자·`_` 밖의 문자가 있거나 숫자로 시작하면 `'1분기 예산'!`.
 * Rust `a1::sheet_prefix`와 같은 규칙. */
export function sheetPrefix(name: string): string {
  const plain = name !== "" && !/^[0-9]/.test(name) && /^[\p{L}\p{N}_.]+$/u.test(name);
  return plain ? `${name}!` : `'${name.replace(/'/g, "''")}'!`;
}

/** `인력계획!F7`, `'1분기 예산'!F7:H7`, `F7` → 시트 이름(적었으면)과 시트 주소 범위. 해석할 수 없으면 `null`.
 * Rust `a1::split_sheet` + `CellRange::parse`와 같은 규칙. */
export function parseSheetRange(text: string): { sheet?: string; range: CellRange } | null {
  const t = text.trim();
  let name: string | undefined;
  let addr = t;
  const quoted = /^'((?:[^']|'')+)'!(.*)$/s.exec(t);
  if (quoted) {
    name = quoted[1]!.replace(/''/g, "'");
    addr = quoted[2]!;
  } else if (!t.startsWith("'")) {
    const i = t.lastIndexOf("!");
    if (i > 0) {
      name = t.slice(0, i);
      addr = t.slice(i + 1);
    }
  }
  const range = parseRange(addr);
  if (!range) return null;
  return name === undefined ? { range } : { sheet: name, range };
}

/** `ref`는 보이는 그대로(시트 이름 포함), `range`는 시트 주소, `sheet`는 적었으면 그 시트 이름. */
export type TextPart = { text: string } | { ref: string; range: CellRange; sheet?: string };

/** 채팅 글의 `#F7`, `#F7:H7`, `#인력계획!F7`, `#'1분기 예산'!F7` 셀 참조를 나눈다(시트 주소). 누르면 그 칸으로 간다. */
export function splitCellRefs(text: string): TextPart[] {
  const out: TextPart[] = [];
  const re = /#(?:'((?:[^']|'')+)'!|([\p{L}\p{N}_.]+)!)?([A-Za-z]{1,3}\d{1,7}(?::[A-Za-z]{1,3}\d{1,7})?)/gu;
  let last = 0;
  for (let m = re.exec(text); m; m = re.exec(text)) {
    const range = parseRange(m[3]!);
    if (!range) continue;
    const sheet = m[1] !== undefined ? m[1].replace(/''/g, "'") : m[2];
    if (m.index > last) out.push({ text: text.slice(last, m.index) });
    const addr = m[3]!.toUpperCase();
    out.push(sheet !== undefined ? { ref: `${sheetPrefix(sheet)}${addr}`, range, sheet } : { ref: addr, range });
    last = m.index + m[0].length;
  }
  if (last < text.length) out.push({ text: text.slice(last) });
  return out;
}
