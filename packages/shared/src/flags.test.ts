import { describe, expect, it } from "vitest";

import { decodeFlags, FLAG, personColor } from "./flags";

describe("셀 상태 비트", () => {
  it("base64 → 바이트", () => {
    expect(Array.from(decodeFlags(btoa(String.fromCharCode(0, 1, 130, 255))))).toEqual([0, 1, 130, 255]);
    expect(FLAG.MINE | FLAG.INPUT).toBe(129);
  });

  it("담당자 색은 순환한다", () => {
    expect(personColor(0)).toBe(personColor(10));
  });
});
