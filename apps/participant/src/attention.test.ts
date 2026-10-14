import { describe, expect, it } from "vitest";

import { alertLabel, blinkTitle } from "./attention";

describe("탭 제목 알림", () => {
  it("알릴 것이 없으면 글이 없다", () => {
    expect(alertLabel({ chat: 0, notice: false })).toBeNull();
  });

  it("새 채팅 수를 센다", () => {
    expect(alertLabel({ chat: 3, notice: false })).toBe("💬 새 메시지 3개");
  });

  it("공지가 먼저 오고 채팅 수를 덧붙인다", () => {
    expect(alertLabel({ chat: 0, notice: true })).toBe("📢 새 공지");
    expect(alertLabel({ chat: 2, notice: true })).toBe("📢 새 공지 · 💬 2");
  });

  it("켜짐·꺼짐 박자로 평소 제목과 번갈아 보인다", () => {
    const base = "예산 신청 · 엑셀 취합";
    expect(blinkTitle(base, { chat: 1, notice: false }, true)).toBe("💬 새 메시지 1개 · 예산 신청 · 엑셀 취합");
    expect(blinkTitle(base, { chat: 1, notice: false }, false)).toBe(base);
    expect(blinkTitle(base, { chat: 0, notice: false }, true)).toBe(base);
  });
});
