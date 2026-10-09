import { describe, expect, it } from "vitest";

import type { InputFormat } from "./generated";
import { editText, excelSerial, MAX_TEXT_CHARS, normalize, serialToDate } from "./validate";

// Rust collect_core::validate 테스트와 같은 사례(서버와 화면이 같은 판정을 해야 한다).
const num = (unit: string): InputFormat => ({ kind: "number", unit });
const text: InputFormat = { kind: "text" };
const date: InputFormat = { kind: "date" };

describe("normalize", () => {
  it("숫자: 쉼표·공백·단위·%", () => {
    expect(normalize(num("천원"), "9,800")).toEqual({ ok: true, value: 9800 });
    expect(normalize(num("천원"), " 9 800천원 ")).toEqual({ ok: true, value: 9800 });
    expect(normalize(num(""), "5%")).toEqual({ ok: true, value: 0.05 });
    expect(normalize(num(""), "-1.5")).toEqual({ ok: true, value: -1.5 });
    expect(normalize(num(""), 3)).toEqual({ ok: true, value: 3 });
    const bad = normalize(num("천원"), "구천팔백");
    expect(bad.ok === false && bad.error).toContain("천원");
    expect(normalize(num(""), true).ok).toBe(false);
    expect(normalize(num(""), "1.2.3").ok).toBe(false);
  });

  it("빈 값은 언제나 지우기", () => {
    for (const f of [num(""), text, date, { kind: "list", items: ["a"] } as InputFormat]) {
      expect(normalize(f, "  ")).toEqual({ ok: true, value: null });
      expect(normalize(f, null)).toEqual({ ok: true, value: null });
    }
  });

  it("목록", () => {
    const f: InputFormat = { kind: "list", items: ["1분기", "연중", "3"] };
    expect(normalize(f, " 연중 ")).toEqual({ ok: true, value: "연중" });
    expect(normalize(f, 3)).toEqual({ ok: true, value: "3" });
    const bad = normalize(f, "5분기");
    expect(bad.ok === false && bad.error).toContain("1분기, 연중");
  });

  it("날짜 → Excel 일련번호", () => {
    expect(excelSerial(1900, 3, 1)).toBe(61);
    expect(excelSerial(2026, 10, 10)).toBe(46305);
    expect(normalize(date, "2026.10.10")).toEqual({ ok: true, value: 46305 });
    expect(normalize(date, "2026/10/10")).toEqual({ ok: true, value: 46305 });
    expect(normalize(date, "2026-02-30").ok).toBe(false);
    expect(normalize(date, "10/10").ok).toBe(false);
    expect(serialToDate(46305)).toBe("2026-10-10");
  });

  it("글자: 끝 줄바꿈 제거, 길이 제한, 오류 값 거절", () => {
    expect(normalize(text, "간담회 4회\r\n")).toEqual({ ok: true, value: "간담회 4회" });
    expect(normalize(text, "가".repeat(MAX_TEXT_CHARS + 1)).ok).toBe(false);
    expect(normalize(text, { err: 2042 }).ok).toBe(false);
  });
});

describe("editText", () => {
  it("숫자는 서식 없이, 날짜 열은 날짜로", () => {
    expect(editText(9800, num("천원"))).toBe("9800");
    expect(editText(46305, date)).toBe("2026-10-10");
    expect(editText(null, text)).toBe("");
    expect(editText("간담회", text)).toBe("간담회");
  });
});
