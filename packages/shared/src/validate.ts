import type { CellValue, InputFormat } from "./generated";

/**
 * 참여자 입력 검사·정규화. Rust `collect_core::validate::normalize`와 같은 규칙이다(서버가 최종 판정한다).
 * 화면은 이것으로 바로 오류를 보여 주고, 서버 응답 전에도 정규화한 값을 그린다.
 */
export const MAX_TEXT_CHARS = 2000;

export type Normalized = { ok: true; value: CellValue } | { ok: false; error: string };

const ok = (value: CellValue): Normalized => ({ ok: true, value });
const fail = (error: string): Normalized => ({ ok: false, error });

function isBlank(v: CellValue): boolean {
  return v === null || v === undefined || (typeof v === "string" && v.trim() === "");
}

export function normalize(format: InputFormat, value: CellValue): Normalized {
  if (isBlank(value)) return ok(null);
  if (typeof value === "object" && value !== null) return fail("오류 값은 입력할 수 없습니다");
  switch (format.kind) {
    case "text":
      if (typeof value !== "string") return ok(value);
      if ([...value].length > MAX_TEXT_CHARS) return fail(`${MAX_TEXT_CHARS}자까지 입력할 수 있습니다`);
      return ok(value.replace(/[\r\n]+$/, ""));
    case "number": {
      if (typeof value === "number" && Number.isFinite(value)) return ok(value);
      const n = typeof value === "string" ? parseNumber(value, format.unit) : null;
      if (n !== null) return ok(n);
      return fail(format.unit ? `숫자만 입력할 수 있습니다(${format.unit} 단위)` : "숫자만 입력할 수 있습니다");
    }
    case "list": {
      const text =
        typeof value === "string" ? value.trim() : typeof value === "number" ? String(value) : value ? "TRUE" : "FALSE";
      const item = format.items.find((i) => i.trim() === text);
      return item !== undefined ? ok(item) : fail(`목록에 있는 값만 고를 수 있습니다: ${format.items.join(", ")}`);
    }
    case "date": {
      if (typeof value === "number" && Number.isFinite(value) && value >= 1) return ok(value);
      const d = typeof value === "string" ? parseDate(value) : null;
      return d !== null ? ok(d) : fail("날짜 형식(예: 2026-10-10)으로 입력해 주세요");
    }
  }
}

function parseNumber(text: string, unit: string): number | null {
  let s = text.trim();
  if (unit && s.endsWith(unit)) s = s.slice(0, -unit.length);
  const percent = s.trimEnd().endsWith("%");
  const cleaned = s.replace(/[, % ]/g, "");
  if (cleaned === "" || !/^[+-]?(\d+\.?\d*|\.\d+)([eE][+-]?\d+)?$/.test(cleaned)) return null;
  const n = Number(cleaned);
  if (!Number.isFinite(n)) return null;
  return percent ? n / 100 : n;
}

function daysInMonth(y: number, m: number): number {
  return new Date(Date.UTC(y, m, 0)).getUTCDate();
}

/** 날짜 → Excel 일련번호(1899-12-30 = 0). */
export function excelSerial(y: number, m: number, d: number): number | null {
  if (!Number.isInteger(y) || m < 1 || m > 12 || d < 1 || d > daysInMonth(y, m)) return null;
  return Math.round((Date.UTC(y, m - 1, d) - Date.UTC(1899, 11, 30)) / 86_400_000);
}

/** Excel 일련번호 → `YYYY-MM-DD`. */
export function serialToDate(serial: number): string {
  const ms = Date.UTC(1899, 11, 30) + Math.floor(serial) * 86_400_000;
  return new Date(ms).toISOString().slice(0, 10);
}

function parseDate(text: string): number | null {
  const parts = text
    .trim()
    .replace(/\.$/, "")
    .split(/[-./]/)
    .map((p) => p.trim());
  if (parts.length !== 3 || parts[0]!.length !== 4 || parts.some((p) => !/^\d+$/.test(p))) return null;
  return excelSerial(Number(parts[0]), Number(parts[1]), Number(parts[2]));
}

/** 편집기에 처음 넣을 글자(F2·더블클릭): 숫자는 서식 없이, 날짜는 `YYYY-MM-DD`. */
export function editText(v: CellValue | undefined, format: InputFormat | undefined): string {
  if (v === undefined || v === null) return "";
  if (typeof v === "number") return format?.kind === "date" ? serialToDate(v) : String(v);
  if (typeof v === "boolean") return v ? "TRUE" : "FALSE";
  if (typeof v === "string") return v;
  return "";
}
