import type { SheetModel } from "@mm/shared";
import { describe, expect, it } from "vitest";

import {
  boxRect,
  buildLayout,
  cellBox,
  GUTTER_W,
  HEADER_H,
  indexAt,
  isHidden,
  nextVisible,
  visibleSpan,
} from "./layout";

function model(partial: Partial<SheetModel> = {}): SheetModel {
  const rows = partial.rows ?? 10;
  const cols = partial.cols ?? 3;
  return {
    workbook: "a.xlsx",
    sheet: "s",
    origin: { row: 0, col: 0 },
    rows,
    cols,
    cells: Array.from({ length: rows * cols }, () => ({ s: 0 })),
    styles: [],
    merges: [],
    colWidths: Array(cols).fill(75), // 100px
    rowHeights: Array(rows).fill(15), // 20px
    headerRows: 0,
    frozenRows: 0,
    frozenCols: 0,
    validations: [],
    hiddenRows: [],
    hiddenCols: [],
    ...partial,
  };
}

describe("buildLayout", () => {
  it("pt를 px로 바꿔 누적 위치를 만든다", () => {
    const l = buildLayout(model());
    expect(Array.from(l.colX)).toEqual([0, 100, 200, 300]);
    expect(l.rowY[10]).toBe(200);
  });

  it("머리글 행은 틀 고정보다 많으면 머리글 기준으로 고정", () => {
    const l = buildLayout(model({ headerRows: 2, frozenRows: 1, frozenCols: 1 }));
    expect([l.frozenRows, l.frozenCols, l.frozenH, l.frozenW]).toEqual([2, 1, 40, 100]);
  });

  it("병합 색인", () => {
    const l = buildLayout(model({ merges: [{ row: 2, col: 0, rows: 5, cols: 1 }] }));
    expect(l.mergeAt[6 * 3 + 0]).toBe(0);
    expect(l.mergeAt[7 * 3 + 0]).toBe(-1);
    expect(cellBox(l, 4, 0)).toEqual({ row: 2, col: 0, rows: 5, cols: 1 });
    expect(boxRect(l, cellBox(l, 4, 0))).toEqual({ left: GUTTER_W, top: HEADER_H + 40, width: 100, height: 100 });
  });
});

describe("indexAt / visibleSpan", () => {
  const offsets = Float64Array.from([0, 20, 40, 60, 80, 100]);

  it("이진 탐색", () => {
    expect([-5, 0, 19, 20, 99, 100, 500].map((p) => indexAt(offsets, p))).toEqual([0, 0, 0, 1, 4, 4, 4]);
  });

  it("고정 영역 뒤부터, overscan 포함", () => {
    expect(visibleSpan(offsets, 1, 20, 0, 50, 0)).toEqual({ first: 1, last: 2 });
    expect(visibleSpan(offsets, 1, 20, 30, 50, 1)).toEqual({ first: 1, last: 4 });
    expect(visibleSpan(offsets, 5, 100, 0, 50, 2)).toEqual({ first: 5, last: 4 });
  });
});

describe("숨긴 행·열", () => {
  it("크기 0인 행은 숨김이고, 이동할 때 건너뛴다", () => {
    const l = buildLayout(model({ rows: 5, rowHeights: [15, 0, 0, 15, 15] }));
    expect([0, 1, 2, 3].map((r) => isHidden(l.rowY, r))).toEqual([false, true, true, false]);
    expect(nextVisible(l.rowY, 0, 1)).toBe(3);
    expect(nextVisible(l.rowY, 3, -1)).toBe(0);
    expect(nextVisible(l.rowY, 4, 1)).toBe(4); // 더 갈 곳 없음
  });

  it("숨긴 행에 걸친 병합은 보이는 높이만 차지한다", () => {
    const l = buildLayout(
      model({ rows: 4, rowHeights: [15, 0, 15, 15], merges: [{ row: 0, col: 0, rows: 3, cols: 1 }] }),
    );
    expect(boxRect(l, cellBox(l, 2, 0)).height).toBe(40);
  });
});
