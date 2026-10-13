import { describe, expect, it } from "vitest";

import { placePopover } from "./place";

describe("팝오버 자리", () => {
  const viewport = { width: 1280, height: 800 };
  const size = { width: 200, height: 120 };

  it("기준 아래, 왼쪽을 맞춘다", () => {
    expect(placePopover({ left: 100, top: 100, width: 40, height: 22 }, size, viewport)).toEqual({
      left: 100,
      top: 128,
      side: "below",
    });
  });

  it("아래가 모자라면 위로 뒤집는다", () => {
    expect(placePopover({ left: 100, top: 700, width: 40, height: 22 }, size, viewport)).toEqual({
      left: 100,
      top: 574,
      side: "above",
    });
  });

  it("오른쪽 끝의 칩은 화면 안으로 당긴다", () => {
    expect(placePopover({ left: 1250, top: 100, width: 20, height: 22 }, size, viewport).left).toBe(1072);
  });

  it("위아래 모두 모자라면 넓은 쪽", () => {
    const tall = { width: 200, height: 700 };
    expect(placePopover({ left: 0, top: 200, width: 40, height: 22 }, tall, viewport).side).toBe("below");
    expect(placePopover({ left: 0, top: 600, width: 40, height: 22 }, tall, viewport)).toMatchObject({
      side: "above",
      top: 8,
    });
  });
});
