import { describe, expect, it } from "vitest";

import { a1, colLetters, formatValue, rgbHex } from "./format";

describe("formatValue", () => {
  it("숫자 표시 형식", () => {
    expect(formatValue(41500, "#,##0")).toBe("41,500");
    expect(formatValue(0.0714, "+0.0%;-0.0%;0.0%")).toBe("+7.1%");
    expect(formatValue(-0.05, "+0.0%;-0.0%;0.0%")).toBe("-5.0%");
    expect(formatValue(0, "+0.0%;-0.0%;0.0%")).toBe("0.0%");
  });

  it("일반 형식은 유효 숫자 11자리", () => {
    expect(formatValue(9800, "General")).toBe("9800");
    expect(formatValue(1 / 3, "General")).toBe("0.33333333333");
    expect(formatValue(1.5, "")).toBe("1.5");
  });

  it("글자·논리·오류·빈 값", () => {
    expect(formatValue("간담회 4회", "General")).toBe("간담회 4회");
    expect(formatValue(true, "General")).toBe("TRUE");
    expect(formatValue({ err: 2042 }, "General")).toBe("#N/A");
    expect(formatValue(null, "#,##0")).toBe("");
    expect(formatValue(undefined, "#,##0")).toBe("");
  });

  it("해석할 수 없는 형식은 일반으로 대체", () => {
    expect(formatValue(12, "[이상한")).toBe("12");
  });
});

describe("주소 유틸", () => {
  it("열 문자", () => {
    expect([0, 25, 26, 51, 52, 701, 702].map(colLetters)).toEqual(["A", "Z", "AA", "AZ", "BA", "ZZ", "AAA"]);
    expect(a1(8, 5)).toBe("F9");
  });

  it("색", () => {
    expect(rgbHex(0xbfbfbf)).toBe("#bfbfbf");
    expect(rgbHex(0xff)).toBe("#0000ff");
  });
});
