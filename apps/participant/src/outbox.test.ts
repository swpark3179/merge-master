import { describe, expect, it } from "vitest";

import { splitBySheet } from "./outbox";

describe("splitBySheet", () => {
  it("저장한 묶음을 시트 이름으로 나눈다(옛 묶음은 첫 시트, 없어진 시트는 버림)", () => {
    const cells = [{ row: 1, col: 0, value: 1, base: 0 }];
    const parts = splitBySheet(
      [
        { batch: "a", cells, sheet: 1, sheetName: "인력계획" },
        { batch: "b", cells },
        { batch: "c", cells, sheet: 2, sheetName: "없어진 시트" },
      ],
      ["인력계획", "예산신청서"],
    );
    expect(parts[0]!.map((b) => [b.batch, b.sheet])).toEqual([
      ["a", 0],
      ["b", 0],
    ]);
    expect(parts[1]).toEqual([]);
  });
});
