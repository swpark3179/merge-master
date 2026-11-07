import { describe, expect, it } from "vitest";

import { formatSeen, formatSeenFull } from "./seen";

describe("마지막 접속 시각", () => {
  const now = new Date(2026, 10, 7, 15, 0);
  const at = (y: number, mo: number, d: number, h: number, mi: number) => new Date(y, mo - 1, d, h, mi).getTime();

  it("오늘·어제는 시각만", () => {
    expect(formatSeen(at(2026, 11, 7, 0, 5), now)).toBe("오늘 00:05");
    expect(formatSeen(at(2026, 11, 7, 14, 32), now)).toBe("오늘 14:32");
    expect(formatSeen(at(2026, 11, 6, 23, 59), now)).toBe("어제 23:59");
  });

  it("그 전은 날짜(요일)와 시각, 다른 해는 연도도", () => {
    expect(formatSeen(at(2026, 11, 5, 9, 0), now)).toBe("11/5(목) 09:00");
    expect(formatSeen(at(2026, 1, 1, 8, 30), now)).toBe("1/1(목) 08:30");
    expect(formatSeen(at(2025, 12, 31, 18, 0), now)).toBe("2025/12/31(수) 18:00");
  });

  it("다른 PC 시계가 조금 빨라도(미래) 날짜로 보인다", () => {
    expect(formatSeen(at(2026, 11, 8, 9, 0), now)).toBe("11/8(일) 09:00");
  });

  it("전체 시각", () => {
    expect(formatSeenFull(at(2026, 11, 7, 14, 32))).toBe("2026-11-07 14:32");
  });
});
