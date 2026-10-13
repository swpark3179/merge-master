import { describe, expect, it } from "vitest";

import { chatEnterKey, reactionGroups } from "./chat";

describe("채팅 리액션", () => {
  const marks = [
    { kind: "good", by: "이서연", host: false },
    { kind: "ok", by: "박지훈", host: true },
    { kind: "good", by: "최도윤", host: false },
  ] as const;

  it("종류 순서로 모으고 단 사람을 단 순서로", () => {
    const g = reactionGroups([...marks], { name: "최도윤" });
    expect(g.map((x) => [x.kind, x.count, x.names, x.mine])).toEqual([
      ["ok", 1, ["박지훈"], false],
      ["good", 2, ["이서연", "최도윤"], true],
    ]);
    expect(g[1]!.emoji).toBe("👍");
    expect(reactionGroups(undefined, { name: "최도윤" })).toEqual([]);
  });

  it("호스트는 이름이 아니라 host로 가린다", () => {
    const host = reactionGroups([...marks], { host: true });
    expect(host.map((x) => x.mine)).toEqual([true, false]);
    // 참여자 이름이 호스트 이름과 같아도 호스트 리액션은 내 것이 아니다.
    expect(reactionGroups([...marks], { name: "박지훈" })[0]!.mine).toBe(false);
  });

  it("단 사람 목록에 호스트·나를 표시한다", () => {
    const g = reactionGroups([...marks], { name: "최도윤" });
    expect(g[0]!.people).toEqual([{ name: "박지훈", host: true, me: false }]);
    expect(g[1]!.people).toEqual([
      { name: "이서연", host: false, me: false },
      { name: "최도윤", host: false, me: true },
    ]);
    expect(reactionGroups([...marks], { host: true })[0]!.people[0]!.me).toBe(true);
  });
});

describe("채팅 입력 Enter", () => {
  const key = (k: Partial<Parameters<typeof chatEnterKey>[0]> & { isComposing?: boolean }) =>
    chatEnterKey({
      key: "Enter",
      shiftKey: false,
      ctrlKey: false,
      metaKey: false,
      altKey: false,
      keyCode: 13,
      ...k,
      nativeEvent: { isComposing: k.isComposing ?? false },
    });

  it("Enter는 보내고, Ctrl·⌘·Alt+Enter는 줄바꿈, Shift+Enter는 textarea에 맡긴다", () => {
    expect(key({})).toBe("send");
    expect(key({ ctrlKey: true })).toBe("newline");
    expect(key({ metaKey: true })).toBe("newline");
    expect(key({ altKey: true })).toBe("newline");
    expect(key({ shiftKey: true })).toBeNull();
    expect(key({ key: "a" })).toBeNull();
  });

  it("한글 조합 중 Enter는 조합 확정", () => {
    expect(key({ isComposing: true })).toBeNull();
    expect(key({ keyCode: 229 })).toBeNull();
  });
});
