import { describe, expect, it } from "vitest";

import { buildRefPreview } from "./preview";

describe("셀 참조 미리보기", () => {
  const src = {
    sheet: "예산신청서",
    origin: { row: 3, col: 0 },
    rows: 10,
    cols: 8,
    text: (r: number, c: number) => ({ text: `${r},${c}`, numeric: c === 5 }),
    label: (c: number) => ["부서", "항목", "", "", "", "금액", "", "근거"][c],
    hidden: (r: number) => r === 4,
  };

  it("시트 주소·크기와 처음 3행 × 4열", () => {
    const p = buildRefPreview(src, { row: 2, col: 4, rows: 6, cols: 4 });
    expect(p.addr).toBe("E6:H11");
    expect(p.size).toBe("6행 × 4열 · 24칸");
    expect(p.heads).toEqual([
      { letter: "E", label: "" },
      { letter: "F", label: "금액" },
      { letter: "G", label: "" },
      { letter: "H", label: "근거" },
    ]);
    expect(p.rows.map((r) => r.n)).toEqual([6, 7, 8]);
    expect(p.rows[0]!.cells[1]).toEqual({ text: "2,5", numeric: true, hidden: false });
    // 남의 행은 값을 감춘다.
    expect(p.rows[2]!.cells.every((c) => c.hidden && c.text === "")).toBe(true);
    expect(p.more).toBe("아래로 3행 더");
  });

  it("값을 모르면 이름·주소·크기만", () => {
    const p = buildRefPreview(
      { sheet: "인력계획", origin: { row: 0, col: 0 }, rows: 5, cols: 5 },
      {
        row: 1,
        col: 3,
        rows: 5,
        cols: 1,
      },
    );
    expect(p).toEqual({ sheet: "인력계획", addr: "D2:D6", size: "5행 × 1열 · 5칸", heads: [], rows: [] });
  });

  it("시트 밖으로 나간 부분은 크기에만 센다", () => {
    const p = buildRefPreview(src, { row: 8, col: 7, rows: 4, cols: 3 });
    expect(p.rows.map((r) => r.cells.length)).toEqual([1, 1]);
    expect(p.more).toBe("아래로 2행 더 · 오른쪽으로 2열 더");
  });
});
