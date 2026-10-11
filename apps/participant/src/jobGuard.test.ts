import { describe, expect, it } from "vitest";

import { isOtherJob } from "./useSession";

describe("같은 주소의 다른 업무", () => {
  it("기억한 업무 id와 새 업무 id가 둘 다 있고 다를 때만", () => {
    expect(isOtherJob(null, "b")).toBe(false); // 처음 들어옴
    expect(isOtherJob("a", undefined)).toBe(false); // 업무 id를 모르는 서버
    expect(isOtherJob("a", "a")).toBe(false);
    expect(isOtherJob("a", "b")).toBe(true);
  });
});
