import type { SheetModel } from "@mm/shared";
import { describe, expect, it } from "vitest";

import { buildLayout } from "./layout";
import { columnSelection, countCells, expandToMerges, rowSelection, selectionRange } from "./selection";

function model(merges: SheetModel["merges"]): SheetModel {
  const rows = 8;
  const cols = 4;
  return {
    workbook: "a.xlsx",
    sheet: "s",
    origin: { row: 0, col: 0 },
    rows,
    cols,
    cells: Array.from({ length: rows * cols }, () => ({ s: 0 })),
    styles: [],
    merges,
    colWidths: Array(cols).fill(60),
    rowHeights: Array(rows).fill(15),
    headerRows: 2,
    frozenRows: 0,
    frozenCols: 0,
    validations: [],
    hiddenRows: [],
    hiddenCols: [],
  };
}

describe("selection", () => {
  // A3:A6 병합, B3:B4 병합, B5:C5 병합
  const l = buildLayout(
    model([
      { row: 2, col: 0, rows: 4, cols: 1 },
      { row: 2, col: 1, rows: 2, cols: 1 },
      { row: 4, col: 1, rows: 1, cols: 2 },
    ]),
  );

  it("걸친 병합을 모두 품도록 넓힌다(연쇄 포함)", () => {
    // B4 하나를 고르면 B3:B4 → 그대로. A4를 고르면 A3:A6.
    expect(expandToMerges(l, { row: 3, col: 1, rows: 1, cols: 1 })).toEqual({ row: 2, col: 1, rows: 2, cols: 1 });
    // A6:B6 → A3:A6을 품고, 넓어진 범위가 B3:B4·B5:C5에 걸쳐 다시 넓어진다.
    expect(expandToMerges(l, { row: 5, col: 0, rows: 1, cols: 2 })).toEqual({ row: 2, col: 0, rows: 4, cols: 3 });
  });

  it("거꾸로 끈 선택도 사각형으로", () => {
    expect(selectionRange(l, { anchor: { row: 7, col: 3 }, focus: { row: 6, col: 2 } })).toEqual({
      row: 6,
      col: 2,
      rows: 2,
      cols: 2,
    });
  });

  it("열·행 선택: 열은 머리글 아래 데이터 행 전체", () => {
    expect(columnSelection(l, 2, 3, 3)).toEqual({ anchor: { row: 2, col: 3 }, focus: { row: 7, col: 3 } });
    expect(rowSelection(l, 6, 7)).toEqual({ anchor: { row: 6, col: 0 }, focus: { row: 7, col: 3 } });
  });

  it("병합은 한 칸으로 센다", () => {
    expect(countCells(l, { row: 2, col: 0, rows: 4, cols: 3 })).toBe(7); // A3·B3·C3·C4·B5·B6·C6
  });
});
