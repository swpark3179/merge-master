import { describe, expect, it } from "vitest";

import { dday, formatDeadline, parseDeadline, remaining } from "./deadline";

describe("마감 표시", () => {
  it("로컬 시각 해석·표시", () => {
    expect(formatDeadline("2026-10-10T18:00")).toBe("10/10(토) 18:00");
    expect(parseDeadline("2026-10-10")?.getHours()).toBe(23); // 날짜만 있으면 그날 끝
    expect(formatDeadline("")).toBe("");
    expect(parseDeadline("내일")).toBeNull();
  });

  it("D-day", () => {
    const now = new Date(2026, 9, 8, 14, 0);
    expect(dday("2026-10-10T18:00", now)).toBe("D-2");
    expect(dday("2026-10-08T18:00", now)).toBe("D-day");
    expect(dday("2026-10-08T09:00", now)).toBe("마감");
    expect(dday(undefined, now)).toBe("");
  });
});

describe("remaining", () => {
  const now = new Date(2026, 9, 8, 15, 0);
  it("남은 시간", () => {
    expect(remaining("2026-10-10T18:00", now)).toBe("2일 3시간");
    expect(remaining("2026-10-08T18:20", now)).toBe("3시간 20분");
    expect(remaining("2026-10-08T15:12", now)).toBe("12분");
    expect(remaining("2026-10-08T14:00", now)).toBe("마감 지남");
    expect(remaining(undefined, now)).toBe("");
  });
});
