import { describe, expect, it } from "vitest";

import { parseTsv, toTsv } from "./clipboard";

describe("TSV", () => {
  it("Excel에서 복사한 범위", () => {
    expect(parseTsv("9800\t간담회 4회\r\n3500\t\r\n")).toEqual([
      ["9800", "간담회 4회"],
      ["3500", ""],
    ]);
  });

  it("한 칸(끝 줄바꿈 없음)", () => {
    expect(parseTsv("9,800")).toEqual([["9,800"]]);
  });

  it("따옴표로 감싼 칸: 줄바꿈·탭·따옴표", () => {
    expect(parseTsv('"첫 줄\n둘째 줄"\t"말 ""인용"""\r\n')).toEqual([["첫 줄\n둘째 줄", '말 "인용"']]);
  });

  it("왕복", () => {
    const rows = [
      ["a", "b\tc"],
      ['"q"', ""],
    ];
    expect(parseTsv(toTsv(rows))).toEqual(rows);
    expect(toTsv([["1", "2"]])).toBe("1\t2\r\n");
  });
});
