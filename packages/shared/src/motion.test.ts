import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

import { FLASH_MS, scrollBehavior } from "./motion";

describe("동작", () => {
  it("편집 플래시 시간이 tokens.css의 --dur-flash와 같다", () => {
    const css = readFileSync(new URL("./tokens.css", import.meta.url), "utf8");
    expect(css).toMatch(new RegExp(`--dur-flash:\\s*${FLASH_MS}ms;`));
  });

  it("처음 스크롤은 바로, 그다음부터 부드럽게", () => {
    expect(scrollBehavior(true)).toBe("auto");
    expect(scrollBehavior(false)).toBe("smooth");
  });
});
