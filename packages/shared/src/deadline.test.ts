import { describe, expect, it } from "vitest";

import {
  dday,
  formatDeadline,
  joinDeadline,
  minuteOptions,
  parseDeadline,
  remaining,
  remainingAt,
  remainingTick,
  splitDeadline,
  toLocalDeadline,
} from "./deadline";

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

describe("remainingAt", () => {
  const due = new Date(2026, 9, 8, 18, 0).getTime();
  it("분은 올려 센다(마감 시각 전에는 '마감 지남'이 아니다)", () => {
    expect(remainingAt(due, due - 90_000)).toBe("2분");
    expect(remainingAt(due, due - 3 * 3_600_000 - 1_000)).toBe("3시간 1분");
    expect(remainingAt(due, due - 3 * 3_600_000)).toBe("3시간");
    expect(remainingAt(due, due - 40_000)).toBe("40초");
    expect(remainingAt(due, due - 1)).toBe("1초");
    expect(remainingAt(due, due)).toBe("마감 지남");
    expect(remainingAt(due, due + 5_000)).toBe("마감 지남");
    expect(remainingAt(undefined, due)).toBe("");
  });

  it("2분 안쪽은 1초마다 다시 그린다", () => {
    expect(remainingTick(due, due - 100_000)).toBe(1_000);
    expect(remainingTick(due, due - 10 * 60_000)).toBe(15_000);
    expect(remainingTick(null, due)).toBe(15_000);
  });
});

describe("마감 입력 조각", () => {
  it("나누기", () => {
    expect(splitDeadline("2026-10-10T18:05")).toEqual({ date: "2026-10-10", hour: "18", minute: "05" });
    expect(splitDeadline("2026-10-10")).toEqual({ date: "2026-10-10", hour: "23", minute: "59" });
    expect(splitDeadline("")).toBeNull();
    expect(splitDeadline("내일")).toBeNull();
  });

  it("합치기: 셋 다 있어야 하고 없는 날은 거절", () => {
    expect(joinDeadline("2026-10-10", "18", "05")).toBe("2026-10-10T18:05");
    expect(joinDeadline("2026-10-10", "9", "0")).toBe("2026-10-10T09:00");
    expect(joinDeadline("", "18", "00")).toBeNull();
    expect(joinDeadline("2026-10-10", "", "00")).toBeNull();
    expect(joinDeadline("2026-10-10", "24", "00")).toBeNull();
    expect(joinDeadline("2026-02-30", "18", "00")).toBeNull();
  });

  it("분 선택지: 간격 + 간격 밖 지금 값", () => {
    expect(minuteOptions(5)).toHaveLength(12);
    expect(minuteOptions(5)[1]).toBe("05");
    const odd = minuteOptions(5, "07");
    expect(odd).toHaveLength(13);
    expect(odd.slice(1, 3)).toEqual(["05", "07"]);
    expect(minuteOptions(5, "10")).toHaveLength(12);
  });

  it("이 기기 시각 글자", () => {
    expect(toLocalDeadline(new Date(2026, 0, 2, 3, 4))).toBe("2026-01-02T03:04");
  });
});
