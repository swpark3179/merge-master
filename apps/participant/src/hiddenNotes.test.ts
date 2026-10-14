import { describe, expect, it } from "vitest";

import { parseHidden, pruneHidden, serializeHidden, toggleHidden } from "./hiddenNotes";

describe("숨긴 강조 메모", () => {
  it("같은 업무면 남긴 번호를 그대로 읽는다", () => {
    const raw = serializeHidden("job-a", new Set([7, 3]));
    expect(raw).toBe('{"job":"job-a","ids":[3,7]}');
    expect([...parseHidden(raw, "job-a")]).toEqual([3, 7]);
  });

  it("다른 업무의 것·깨진 글은 버린다", () => {
    const raw = serializeHidden("job-a", new Set([1]));
    expect(parseHidden(raw, "job-b").size).toBe(0);
    expect(parseHidden("{not json", "job-a").size).toBe(0);
    expect(parseHidden('{"job":"job-a","ids":"1"}', "job-a").size).toBe(0);
    expect([...parseHidden('{"job":"job-a","ids":[1,"x",2.5,4]}', "job-a")]).toEqual([1, 4]);
    expect(parseHidden(null, "job-a").size).toBe(0);
  });

  it("해제된 강조는 잊고, 바뀐 것이 없으면 같은 집합", () => {
    const ids = new Set([1, 2, 3]);
    expect([...pruneHidden(ids, [1, 3, 9])]).toEqual([1, 3]);
    expect(pruneHidden(ids, [1, 2, 3, 4])).toBe(ids);
  });

  it("숨기고 다시 보인다", () => {
    const once = toggleHidden(new Set(), 5);
    expect([...once]).toEqual([5]);
    expect(toggleHidden(once, 5).size).toBe(0);
  });
});
