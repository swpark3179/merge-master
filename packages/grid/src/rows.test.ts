import type { SheetModel } from "@mm/shared";
import { describe, expect, it } from "vitest";

import { fitRowHeights, withHiddenRows } from "./rows";

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
