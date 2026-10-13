import { describe, expect, it } from "vitest";

import {
  cellRefSpans,
  cellRefToken,
  colSpanLabel,
  formatRange,
  parseRange,
  parseSheetRange,
  sheetPrefix,
  splitCellRefs,
} from "./a1";

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
    // 한국어 조사가 바로 붙어도 참조만 떼어 낸다.
    expect(splitCellRefs("#F7에 적어 주세요")[0]).toEqual({ ref: "F7", range: { row: 6, col: 5, rows: 1, cols: 1 } });
  });

  it("시트 이름이 붙은 참조", () => {
    expect(splitCellRefs("#인력계획!e5 와 #'1분기 예산'!F7:H7")).toEqual([
      { ref: "인력계획!E5", range: { row: 4, col: 4, rows: 1, cols: 1 }, sheet: "인력계획" },
      { text: " 와 " },
      { ref: "'1분기 예산'!F7:H7", range: { row: 6, col: 5, rows: 1, cols: 3 }, sheet: "1분기 예산" },
    ]);
    expect(splitCellRefs("#'Bob''s'!A1")[0]).toMatchObject({ sheet: "Bob's", ref: "'Bob''s'!A1" });
  });

  it("위치와 함께 찾는다", () => {
    const text = "근거는 #예산신청서!F2:F9 와 #h10";
    const spans = cellRefSpans(text);
    expect(spans.map((s) => text.slice(s.start, s.end))).toEqual(["#예산신청서!F2:F9", "#h10"]);
    expect(spans[1]).toMatchObject({ ref: "H10", range: { row: 9, col: 7, rows: 1, cols: 1 } });
    expect(spans[1]!.sheet).toBeUndefined();
  });

  it("채팅에 넣는 참조 글자는 시트 이름을 늘 붙인다", () => {
    expect(cellRefToken("예산신청서", { row: 5, col: 5, rows: 3, cols: 3 })).toBe("#예산신청서!F6:H8");
    expect(cellRefToken("1분기 예산", { row: 6, col: 5, rows: 1, cols: 1 })).toBe("#'1분기 예산'!F7");
    // 만든 글자는 다시 같은 참조로 읽힌다.
    expect(splitCellRefs(cellRefToken("1분기 예산", { row: 6, col: 5, rows: 1, cols: 3 }))).toEqual([
      { ref: "'1분기 예산'!F7:H7", range: { row: 6, col: 5, rows: 1, cols: 3 }, sheet: "1분기 예산" },
    ]);
  });

  it("시트 이름이 붙은 범위", () => {
    expect(parseSheetRange("F7:H7")).toEqual({ range: { row: 6, col: 5, rows: 1, cols: 3 } });
    expect(parseSheetRange("인력계획!E5")).toEqual({ sheet: "인력계획", range: { row: 4, col: 4, rows: 1, cols: 1 } });
    expect(parseSheetRange("'1분기 예산'!F7")).toMatchObject({ sheet: "1분기 예산" });
    expect(parseSheetRange("'Bob''s'!A1")).toMatchObject({ sheet: "Bob's" });
    expect(parseSheetRange("인력계획!")).toBeNull();
    expect(parseSheetRange("'열린 따옴표!A1")).toBeNull();
  });

  it("시트 이름 접두(Rust와 같은 규칙)", () => {
    expect(sheetPrefix("인력계획")).toBe("인력계획!");
    expect(sheetPrefix("1분기 예산")).toBe("'1분기 예산'!");
    expect(sheetPrefix("Bob's")).toBe("'Bob''s'!");
  });
});
