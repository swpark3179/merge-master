import type { SessionListItem } from "@mm/shared";
import { describe, expect, it } from "vitest";

import { sessionHref, sessionState, sortSessions } from "./picker";

const item = (slug: string, mine = false, extra: Partial<SessionListItem> = {}): SessionListItem => ({
  slug,
  title: slug,
  requester: "",
  description: "",
  readOnly: false,
  closed: false,
  online: 0,
  mine,
  ...extra,
});

describe("업무 목록", () => {
  it("내 담당을 먼저, 나머지는 공개한 순서대로", () => {
    const sorted = sortSessions([item("a"), item("b", true), item("c"), item("d", true)]);
    expect(sorted.map((s) => s.slug)).toEqual(["b", "d", "a", "c"]);
  });

  it("주소는 경로 조각으로 인코딩한다", () => {
    expect(sessionHref("q4")).toBe("/q4");
    expect(sessionHref("a b")).toBe("/a%20b");
  });

  it("마감 상태", () => {
    expect(sessionState(item("a"))).toBeNull();
    expect(sessionState(item("a", false, { readOnly: true }))).toBe("마감 시각 지남");
    expect(sessionState(item("a", false, { readOnly: true, closed: true }))).toBe("취합 마감됨");
  });
});
