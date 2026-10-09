import { describe, expect, it } from "vitest";

import { colSpanLabel, formatRange, parseRange, splitCellRefs } from "./a1";

describe("parseRange / formatRange", () => {
  it("Rust CellRange::parse와 같은 결과", () => {
    expect(parseRange("$B$3:$B$7")).toEqual({ row: 2, col: 1, rows: 5, cols: 1 });
    expect(parseRange("D7:B3")).toEqual({ row: 2, col: 1, rows: 5, cols: 3 });
    expect(parseRange("f6")).toEqual({ row: 5, col: 5, rows: 1, cols: 1 });
    expect(parseRange(" A1:I1002 ")).toEqual({ row: 0, col: 0, rows: 1002, cols: 9 });
  });

  it("잘못된 입력은 null", () => {
    for (const bad of ["", "A", "A0", "1:2", "A1:B2:C3", "A1:", "가1"]) expect(parseRange(bad)).toBeNull();
  });

  it("왕복", () => {
    for (const a of ["A4:G25", "F6", "AA10:AB12"]) expect(formatRange(parseRange(a)!)).toBe(a);
  });

  it("열 범위 이름", () => {
    expect(colSpanLabel(0, 4)).toBe("A–D열");
    expect(colSpanLabel(2, 1)).toBe("C열");
    expect(colSpanLabel(0, 0)).toBe("없음");
  });
});

describe("splitCellRefs", () => {
  it("#셀 참조를 나눈다", () => {
    const parts = splitCellRefs("복원했습니다 #F7:H7 확인 #h10.");
    expect(parts).toEqual([
      { text: "복원했습니다 " },
      { ref: "F7:H7", range: { row: 6, col: 5, rows: 1, cols: 3 } },
      { text: " 확인 " },
      { ref: "H10", range: { row: 9, col: 7, rows: 1, cols: 1 } },
      { text: "." },
    ]);
    expect(splitCellRefs("참조 없음")).toEqual([{ text: "참조 없음" }]);
  });
});
