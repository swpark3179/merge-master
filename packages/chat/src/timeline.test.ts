import { describe, expect, it } from "vitest";

import { firstUnread } from "./timeline";

describe("안 읽은 첫 글", () => {
  const items = [
    { id: 3, mine: false },
    { id: 5, mine: true },
    { id: 6, mine: false },
    { id: 8, mine: false, counts: false },
  ];

  it("본 글 다음의 첫 남의 글", () => {
    expect(firstUnread(items, 3)).toBe(6);
    expect(firstUnread(items, 0)).toBe(3);
  });

  it("내 글과 세지 않는 글은 건너뛰고, 없으면 null", () => {
    expect(firstUnread(items, 6)).toBeNull();
    expect(firstUnread([{ id: 9, mine: true, counts: true }], 0)).toBe(9);
  });
});
