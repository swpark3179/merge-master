import type { SheetModel } from "@mm/shared";
import { describe, expect, it } from "vitest";

import { resizeRuns, resizeSpan, withLineSize } from "./sizes";

const model: SheetModel = {
  workbook: "w",
  sheet: "s",
  origin: { row: 0, col: 0 },
  rows: 6,
  cols: 4,
  cells: [],
  styles: [],
  merges: [],
  colWidths: [60, 60, 60, 60],
  rowHeights: [15, 15, 15, 0, 15, 15],
  headerRows: 1,
  frozenRows: 0,
  frozenCols: 0,
  validations: [],
  hiddenRows: [3],
  hiddenCols: [],
};

describe("크기 조절", () => {
  it("머리글로 고른 열·행은 함께, 아니면 끈 줄 하나", () => {
    const cols = { row: 1, col: 1, rows: 5, cols: 2 }; // 열 머리글 클릭 = 데이터 행 전체
    expect(resizeSpan(model, cols, "col", 2)).toEqual({ from: 1, count: 2 });
    expect(resizeSpan(model, cols, "col", 3)).toEqual({ from: 3, count: 1 }); // 고른 열 밖
    expect(resizeSpan(model, cols, "row", 2)).toEqual({ from: 2, count: 1 });
    const rows = { row: 2, col: 0, rows: 3, cols: 4 }; // 행 머리글
    expect(resizeSpan(model, rows, "row", 4)).toEqual({ from: 2, count: 3 });
    expect(resizeSpan(model, { row: 2, col: 1, rows: 1, cols: 1 }, "col", 1)).toEqual({ from: 1, count: 1 });
    expect(resizeSpan(model, null, "row", 5)).toEqual({ from: 5, count: 1 });
  });

  it("숨은 줄(필터·남의 행 숨김)을 건너뛰어 보이는 구간으로 나눈다", () => {
    const rows = { row: 1, col: 0, rows: 5, cols: 4 }; // 2~6행, 4행(3)은 숨김
    expect(resizeRuns(model, rows, "row", 2)).toEqual([
      { from: 1, count: 2 },
      { from: 4, count: 2 },
    ]);
    expect(resizeRuns(model, { row: 1, col: 1, rows: 5, cols: 3 }, "col", 2)).toEqual([{ from: 1, count: 3 }]);
    expect(resizeRuns(model, null, "row", 2)).toEqual([{ from: 2, count: 1 }]);
  });

  it("숨긴 줄은 그대로 두고 새 시트를 돌려준다", () => {
    const out = withLineSize(model, "row", 2, 3, 30);
    expect(out.rowHeights).toEqual([15, 15, 30, 0, 30, 15]);
    expect(model.rowHeights[2]).toBe(15); // 원래 시트는 그대로
    expect(out.colWidths).toBe(model.colWidths);
    expect(withLineSize(model, "col", 3, 9, 80).colWidths).toEqual([60, 60, 60, 80]); // 표 밖은 자른다
  });
});
