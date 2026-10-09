import type { ColumnRule, SheetModel } from "@mm/shared";
import { FLAG } from "@mm/shared";
import { describe, expect, it } from "vitest";

import type { EditContext } from "./gridOps";
import { canEdit, clearRange, copyRange, pasteRange } from "./gridOps";

const MINE = FLAG.INPUT | FLAG.MINE;

// 머리글 1행 + 데이터 3행 × 3열. B·C열이 내 칸(숫자·글자), A열은 입력 금지.
function ctx(over: Partial<EditContext> = {}): EditContext {
  const model: SheetModel = {
    workbook: "a.xlsx",
    sheet: "s",
    origin: { row: 0, col: 0 },
    rows: 4,
    cols: 3,
    cells: Array.from({ length: 12 }, () => ({ s: 0 })),
    styles: [],
    merges: [],
    colWidths: [60, 60, 60],
    rowHeights: [15, 15, 15, 15],
    headerRows: 1,
    frozenRows: 1,
    frozenCols: 0,
    validations: [],
    hiddenRows: [],
    hiddenCols: [],
  };
  const flags = new Uint8Array(12);
  for (let r = 1; r < 4; r++) {
    flags[r * 3] = 0;
    flags[r * 3 + 1] = MINE;
    flags[r * 3 + 2] = MINE;
  }
  flags[0] = flags[1] = flags[2] = FLAG.HEADER;
  const rule = (format: ColumnRule["format"]): ColumnRule => ({
    role: "input",
    required: false,
    filter: false,
    format,
    hint: "",
  });
  return {
    model,
    flags,
    columns: [rule({ kind: "text" }), rule({ kind: "number", unit: "천원" }), rule({ kind: "text" })],
    locks: new Map(),
    hiddenRows: new Set(),
    hiddenCols: new Set(),
    readOnly: false,
    ...over,
  };
}

const range = (row: number, col: number, rows: number, cols: number) => ({ row, col, rows, cols });

describe("gridOps", () => {
  it("편집 가능: 내 칸, 마감 전, 남이 편집 중이 아님", () => {
    expect(canEdit(ctx(), 1, 1)).toBe(true);
    expect(canEdit(ctx(), 1, 0)).toBe(false);
    expect(canEdit(ctx({ readOnly: true }), 1, 1)).toBe(false);
    expect(canEdit(ctx({ locks: new Map([[4, "이서연"]]) }), 1, 1)).toBe(false);
  });

  it("Excel 표를 붙이면 정규화하고, 형식이 틀리거나 남의 칸은 건너뛴다", () => {
    const r = pasteRange(ctx(), range(1, 0, 1, 1), [
      ["x", "9,800", "간담회"],
      ["y", "구천", "비품"],
    ]);
    expect(r.changes).toEqual([
      { row: 1, col: 1, value: 9800 },
      { row: 1, col: 2, value: "간담회" },
      { row: 2, col: 2, value: "비품" },
    ]);
    expect(r.skipped).toBe(2); // A열
    expect(r.invalid).toBe(1);
    expect(r.firstError).toContain("숫자");
  });

  it("한 칸을 범위에 붙이면 범위를 채운다", () => {
    const r = pasteRange(ctx(), range(1, 1, 3, 1), [["5"]]);
    expect(r.changes.map((c) => [c.row, c.value])).toEqual([
      [1, 5],
      [2, 5],
      [3, 5],
    ]);
  });

  it("숨긴 행은 건너뛰고 다음 보이는 행에 붙인다", () => {
    const r = pasteRange(ctx({ hiddenRows: new Set([2]) }), range(1, 1, 1, 1), [["1"], ["2"]]);
    expect(r.changes.map((c) => c.row)).toEqual([1, 3]);
  });

  it("지우기·복사", () => {
    const c = ctx();
    c.model.cells[3]!.v = "부서";
    const r = clearRange(c, range(1, 0, 2, 3));
    expect(r.changes.length).toBe(4);
    expect(r.skipped).toBe(1); // 값이 있는 입력 금지 칸
    expect(copyRange(c, range(1, 0, 2, 2), (row, col) => `${row}${col}`)).toEqual([
      ["10", "11"],
      ["20", "21"],
    ]);
  });
});
