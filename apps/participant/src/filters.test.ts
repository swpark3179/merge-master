import type { SheetModel } from "@mm/shared";
import { FLAG } from "@mm/shared";
import { describe, expect, it } from "vitest";

import { effectiveColumn, filteredOut, filterOptions, missingOf, myRowsOf, NO_FILTER, withHiddenRows } from "./filters";

// 머리글 1행 + 데이터 4행, A열 '본부'는 A2:A3 병합, B열 입력(필수).
function sheet(): SheetModel {
  const v = [
    ["본부", "금액"],
    ["경영지원", null],
    [null, 3],
    ["영업", null],
    ["영업", 5],
  ];
  return {
    workbook: "a.xlsx",
    sheet: "s",
    origin: { row: 0, col: 0 },
    rows: 5,
    cols: 2,
    cells: v.flat().map((x) => ({ v: x, s: 0 })),
    styles: [],
    merges: [{ row: 1, col: 0, rows: 2, cols: 1 }],
    colWidths: [60, 60],
    rowHeights: [15, 15, 15, 15, 15],
    headerRows: 1,
    frozenRows: 1,
    frozenCols: 0,
    validations: [],
    hiddenRows: [],
    hiddenCols: [],
  };
}

const MINE_REQ = FLAG.INPUT | FLAG.REQUIRED | FLAG.MINE;

describe("filters", () => {
  it("병합을 고려한 값과 조회조건 항목", () => {
    const m = sheet();
    expect(effectiveColumn(m, 0)).toEqual(["본부", "경영지원", "경영지원", "영업", "영업"]);
    expect(filterOptions(m, 0)).toEqual(["경영지원", "영업"]);
  });

  it("조회조건·내 담당만·필수 미입력만", () => {
    const m = sheet();
    // 1·2행만 내 칸(B열)
    const flags = new Uint8Array(10);
    flags[3] = MINE_REQ;
    flags[5] = MINE_REQ;
    flags[7] = FLAG.INPUT | FLAG.REQUIRED;
    const mine = myRowsOf(m, flags, [-1, -1, -1, 0, 0], 2);
    expect([...mine]).toEqual([0, 1, 1, 0, 0]);
    expect(missingOf(m, flags, 1)).toBe(1);
    expect(missingOf(m, flags, 2)).toBe(0);

    expect(filteredOut(m, flags, mine, NO_FILTER).size).toBe(0);
    expect([...filteredOut(m, flags, mine, { ...NO_FILTER, values: { 0: "영업" } })]).toEqual([1, 2]);
    expect([...filteredOut(m, flags, mine, { ...NO_FILTER, mineOnly: true })]).toEqual([3, 4]);
    expect([...filteredOut(m, flags, mine, { ...NO_FILTER, missingOnly: true })]).toEqual([2, 3, 4]);
  });

  it("걸러진 행은 높이 0", () => {
    const m = sheet();
    const v = withHiddenRows(m, new Set([2]));
    expect(v.rowHeights).toEqual([15, 15, 0, 15, 15]);
    expect(v.hiddenRows).toEqual([2]);
    expect(v.cells).toBe(m.cells); // 값은 같은 배열(입력이 바로 보인다)
    expect(withHiddenRows(m, new Set())).toBe(m);
  });
});
