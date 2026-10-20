import type { SheetModel } from "@mm/shared";
import { describe, expect, it } from "vitest";

import { cutMerge, fitRowHeights, mergeValueCell, partlyShownMerges, withHiddenRows } from "./rows";

function model(values: (string | number | null)[][], merges: SheetModel["merges"] = []): SheetModel {
  const rows = values.length;
  const cols = values[0]!.length;
  return {
    workbook: "a.xlsx",
    sheet: "s",
    origin: { row: 0, col: 0 },
    rows,
    cols,
    cells: values.flat().map((v) => (v === null ? { s: 0 } : { v, s: 0 })),
    styles: [
      {
        fontName: "",
        fontSize: 10,
        bold: false,
        italic: false,
        underline: false,
        color: 0,
        numberFormat: "General",
        hAlign: "general",
        vAlign: "bottom",
        wrap: false,
        borders: {},
      },
    ],
    merges,
    colWidths: Array(cols).fill(60),
    rowHeights: Array(rows).fill(15),
    headerRows: 0,
    frozenRows: 0,
    frozenCols: 0,
    validations: [],
    hiddenRows: [],
    hiddenCols: [],
  };
}

describe("fitRowHeights", () => {
  it("grows rows whose cells have line breaks, never shrinks", () => {
    const m = model([
      ["a", "첫 줄\n둘째 줄\n셋째 줄"],
      ["b", 3],
      ["c\nd", null],
    ]);
    const fit = fitRowHeights(m);
    expect(fit.rowHeights).toEqual([42, 15, 29]); // 3줄 × 10pt × 1.25 + 4, 2줄
    expect(m.rowHeights).toEqual([15, 15, 15]); // 원본은 그대로
    const tall = { ...m, rowHeights: [60, 15, 15] };
    expect(fitRowHeights(tall).rowHeights[0]).toBe(60);
  });

  it("returns the same model when nothing changes and skips hidden rows and tall merges", () => {
    const flat = model([["a", "b"]]);
    expect(fitRowHeights(flat)).toBe(flat);
    const hidden = withHiddenRows(model([["x\ny", ""]]), new Set([0]));
    expect(fitRowHeights(hidden).rowHeights[0]).toBe(0);
    const merged = model(
      [
        ["x\ny\nz", ""],
        ["", ""],
      ],
      [{ row: 0, col: 0, rows: 2, cols: 1 }],
    );
    expect(fitRowHeights(merged).rowHeights).toEqual([15, 15]);
  });

  it("caps very long text", () => {
    const m = model([[Array.from({ length: 40 }, (_, i) => `줄 ${i}`).join("\n")]]);
    expect(fitRowHeights(m, 8).rowHeights[0]).toBe(104);
  });
});

describe("partlyShownMerges", () => {
  // A1:A3 · B1:B2 · B3:B4 병합, 3행은 원래 숨긴 행
  const m = {
    ...model(
      [
        ["기획", "가"],
        [null, null],
        [null, "나"],
        ["c", null],
      ],
      [
        { row: 0, col: 0, rows: 3, cols: 1 },
        { row: 0, col: 1, rows: 2, cols: 1 },
        { row: 2, col: 1, rows: 2, cols: 1 },
      ],
    ),
    hiddenRows: [2],
  };

  it("finds merges that the filter cuts, keyed by the top-left cell", () => {
    expect(partlyShownMerges(m, new Set())).toEqual(new Map());
    // 1행을 거르면 A1:A3(2행이 보인다)·B1:B2만 걸친다. B3:B4는 그대로 다 보인다(3행은 원래 숨김).
    expect([...partlyShownMerges(m, new Set([0])).keys()]).toEqual([0, 1]);
    // 2행을 거르면 A1:A3·B1:B2가 일부만 보인다.
    expect([...partlyShownMerges(m, new Set([1])).keys()]).toEqual([0, 1]);
    // 4행을 거르면 B3:B4는 원래 숨긴 3행만 남아 보이는 데가 없다.
    expect(partlyShownMerges(m, new Set([3]))).toEqual(new Map());
    // 다 거르면 보이는 데가 없다.
    expect(partlyShownMerges(m, new Set([0, 1, 3]))).toEqual(new Map());
  });

  it("takes the value from the cell that has one", () => {
    expect(mergeValueCell(m, m.merges[0]!)).toBe(0); // 왼쪽 위 칸
    expect(mergeValueCell(m, m.merges[2]!)).toBe(5); // B3
    const loose = model(
      [
        [null, null],
        [null, 7],
      ],
      [{ row: 0, col: 0, rows: 2, cols: 2 }],
    );
    expect(mergeValueCell(loose, loose.merges[0]!)).toBe(3); // 왼쪽 위가 비면 영역 안에서 값이 있는 칸
    const empty = model([[null], [null]], [{ row: 0, col: 0, rows: 2, cols: 1 }]);
    expect(mergeValueCell(empty, empty.merges[0]!)).toBe(0);
  });
});

describe("cutMerge", () => {
  const m = model(
    [["a"], ["b"], ["c"], ["d"]],
    [
      { row: 1, col: 0, rows: 2, cols: 1 },
      { row: 3, col: 0, rows: 1, cols: 1 },
    ],
  );
  it("병합의 첫 행만 지우면 그 병합", () => {
    expect(cutMerge(m, 1, 1)).toEqual({ row: 1, col: 0, rows: 2, cols: 1 });
    expect(cutMerge(m, 0, 2)).toEqual({ row: 1, col: 0, rows: 2, cols: 1 });
  });
  it("병합을 통째로·아래 행만 지우면 없음(한 칸 병합은 보지 않는다)", () => {
    expect(cutMerge(m, 1, 2)).toBeNull();
    expect(cutMerge(m, 2, 1)).toBeNull();
    expect(cutMerge(m, 3, 1)).toBeNull();
  });
});
