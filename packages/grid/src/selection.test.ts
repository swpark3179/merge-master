import type { SheetModel } from "@mm/shared";
import { describe, expect, it } from "vitest";

import { buildLayout } from "./layout";
import {
  columnSelection,
  countCells,
  expandToMerges,
  lineSelection,
  rowSelection,
  selectForMenu,
  selectionRange,
} from "./selection";

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

  it("오른쪽 클릭: 선택 안이면 그대로, 밖이면 그 칸·열·행", () => {
    const m = { rows: 8, cols: 4, headerRows: 2 };
    expect(lineSelection(m, "col", 1, 2)).toEqual(columnSelection(l, 2, 1, 2));
    expect(lineSelection(m, "row", 3)).toEqual(rowSelection(l, 3, 3));
    const cols = lineSelection(m, "col", 1, 2);
    expect(selectForMenu(m, cols, { kind: "col", col: 2 })).toBe(cols);
    expect(selectForMenu(m, cols, { kind: "col", col: 3 })).toEqual(lineSelection(m, "col", 3));
    expect(selectForMenu(m, cols, { kind: "row", row: 4 })).toEqual(lineSelection(m, "row", 4));
    expect(selectForMenu(m, cols, { kind: "cell", row: 4, col: 1 })).toBe(cols);
    expect(selectForMenu(m, null, { kind: "cell", row: 4, col: 1 })).toEqual({
      anchor: { row: 4, col: 1 },
      focus: { row: 4, col: 1 },
    });
    const cell = { anchor: { row: 4, col: 1 }, focus: { row: 4, col: 1 } };
    // 칸만 고른 행은 행 전체로
    expect(selectForMenu(m, cell, { kind: "row", row: 4 })).toEqual(lineSelection(m, "row", 4));
  });
});
