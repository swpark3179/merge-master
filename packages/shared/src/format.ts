import SSF from "ssf";

import type { CellValue, InputFormat } from "./generated";
import { serialToDate } from "./validate";

/** Excel 오류 코드(xlErr) → 표시 문자열. */
const ERRORS: Record<number, string> = {
  2000: "#NULL!",
  2007: "#DIV/0!",
  2015: "#VALUE!",
  2023: "#REF!",
  2029: "#NAME?",
  2036: "#NUM!",
  2042: "#N/A",
};

/** 1pt = 4/3px (96dpi 기준). Excel 열 너비·행 높이·글꼴 크기는 pt로 온다. */
export const PT_TO_PX = 4 / 3;

export function isGeneralFormat(numberFormat: string): boolean {
  return numberFormat === "" || numberFormat === "General" || numberFormat === "@";
}

/** Excel '일반' 형식처럼 유효 숫자 11자리까지 보여 준다. */
function formatGeneral(n: number): string {
  if (Number.isInteger(n)) return String(n);
  return String(Number(n.toPrecision(11)));
}

/** 셀 값을 Excel 표시 형식대로 문자열로 만든다. */
export function formatValue(v: CellValue | undefined, numberFormat: string): string {
  if (v === undefined || v === null) return "";
  if (typeof v === "string") return v;
  if (typeof v === "boolean") return v ? "TRUE" : "FALSE";
  if (typeof v === "number") {
    if (isGeneralFormat(numberFormat)) return formatGeneral(v);
    try {
      return SSF.format(numberFormat, v);
    } catch {
      return formatGeneral(v);
    }
  }
  return ERRORS[v.err] ?? "#ERR";
}

export function isNumberValue(v: CellValue | undefined): v is number {
  return typeof v === "number";
}

/** `0xRRGGBB` → `#rrggbb`. */
export function rgbHex(rgb: number): string {
  return `#${rgb.toString(16).padStart(6, "0")}`;
}

/** 색 코드 → `0xRRGGBB`: `#ff00ff`·`ff00ff`·`#f0f`(앞뒤 공백 무시, 대소문자 무관). 색 코드가 아니면 `null`. */
export function parseHex(text: string): number | null {
  const m = /^#?([0-9a-f]{3}|[0-9a-f]{6})$/i.exec(text.trim());
  if (!m) return null;
  const h = m[1]!.length === 3 ? [...m[1]!].map((c) => c + c).join("") : m[1]!;
  return parseInt(h, 16);
}

/** `0 → "A"`, `26 → "AA"`. */
export function colLetters(col: number): string {
  let s = "";
  let n = col;
  for (;;) {
    s = String.fromCharCode(65 + (n % 26)) + s;
    if (n < 26) return s;
    n = Math.floor(n / 26) - 1;
  }
}

/** 0부터 센 좌표 → `"F9"`. */
export function a1(row: number, col: number): string {
  return `${colLetters(col)}${row + 1}`;
}

/** 열 입력 형식을 고려한 표시: 날짜 열인데 표시 형식이 '일반'이면 일련번호 대신 날짜로 보여 준다. */
export function displayValue(v: CellValue | undefined, numberFormat: string, input?: InputFormat): string {
  if (typeof v === "number" && input?.kind === "date" && isGeneralFormat(numberFormat)) return serialToDate(v);
  return formatValue(v, numberFormat);
}
