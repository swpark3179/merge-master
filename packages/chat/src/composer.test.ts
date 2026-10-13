import { describe, expect, it } from "vitest";

import { deleteToken, insertToken, mirrorSegments, pickInput, pickQuery, pickTrigger } from "./composer";

describe("@ 셀 선택 시작", () => {
  it("맨 앞·공백·줄바꿈 뒤의 @ 한 글자", () => {
    expect(pickTrigger("", "@", 1)).toBe(0);
    expect(pickTrigger("확인 ", "확인 @", 4)).toBe(3);
    expect(pickTrigger("첫 줄\n", "첫 줄\n@", 5)).toBe(4);
    // 가운데에 끼워 넣어도 된다.
    expect(pickTrigger("가 나", "가 @나", 3)).toBe(2);
  });

  it("붙여 쓴 @·붙여넣기·다른 글자는 아니다", () => {
    expect(pickTrigger("kim", "kim@", 4)).toBeNull();
    expect(pickTrigger("", "@F6", 3)).toBeNull();
    expect(pickTrigger("", "a", 1)).toBeNull();
    expect(pickTrigger("ab", "a@", 2)).toBeNull();
  });
});

describe("선택 모드에서 글이 바뀌면", () => {
  it("질의는 @ 뒤 공백 앞까지", () => {
    expect(pickQuery("확인 @F6 부탁", 3)).toBe("F6");
    expect(pickQuery("@", 0)).toBe("");
  });

  it("주소를 치는 동안은 계속하고 미리 보인다", () => {
    expect(pickInput("@", 0, 1)).toEqual({ kind: "keep", range: null });
    expect(pickInput("@F", 0, 2)).toEqual({ kind: "keep", range: null });
    expect(pickInput("@f6:h8", 0, 6)).toEqual({ kind: "keep", range: { row: 5, col: 5, rows: 3, cols: 3 } });
  });

  it("주소 뒤 띄어쓰기는 넣기, 그 밖의 글자·@ 지우기는 끝", () => {
    expect(pickInput("@E2:E9 ", 0, 7)).toEqual({ kind: "commit", range: { row: 1, col: 4, rows: 8, cols: 1 } });
    expect(pickInput("@ ", 0, 2)).toEqual({ kind: "exit" });
    expect(pickInput("@김", 0, 2)).toEqual({ kind: "exit" });
    expect(pickInput("@F ", 0, 3)).toEqual({ kind: "exit" });
    expect(pickInput("abc", 0, 0)).toEqual({ kind: "exit" });
  });
});

describe("참조 넣기·지우기", () => {
  it("@와 질의를 참조로 바꾸고 띄어쓰기 하나, 커서는 그 뒤", () => {
    expect(insertToken("출장비 @", 4, "#예산신청서!F6:H8", 100)).toEqual({
      text: "출장비 #예산신청서!F6:H8 ",
      caret: 17,
    });
    expect(insertToken("@F6 확인", 0, "#예산신청서!F6", 100)).toEqual({ text: "#예산신청서!F6 확인", caret: 9 });
    expect(insertToken("@", 0, "#예산신청서!F6", 100, ",")).toEqual({ text: "#예산신청서!F6,", caret: 10 });
    expect(insertToken("@", 0, "#예산신청서!F6", 5)).toBeNull();
  });

  it("고르기로 넣은 참조만 통째로 지운다", () => {
    const text = "봐 주세요 #예산신청서!F6:H8 그리고 #F9";
    const tokens = ["#예산신청서!F6:H8"];
    expect(deleteToken(text, 18, "Backspace", tokens)).toEqual({ text: "봐 주세요  그리고 #F9", caret: 6 });
    expect(deleteToken(text, 6, "Delete", tokens)).toEqual({ text: "봐 주세요  그리고 #F9", caret: 6 });
    expect(deleteToken(text, text.length, "Backspace", tokens)).toBeNull();
    // 같은 글자로 시작하는 더 긴 참조는 다른 참조다.
    expect(deleteToken("#예산신청서!F6:H8", 12, "Backspace", ["#예산신청서!F6"])).toBeNull();
  });
});

describe("입력 칸 거울", () => {
  it("셀 참조와 선택 모드의 @를 나눈다", () => {
    expect(mirrorSegments("봐 #F6 와 @", 8)).toEqual([
      { text: "봐 ", kind: "text" },
      { text: "#F6", kind: "ref" },
      { text: " 와 ", kind: "text" },
      { text: "@", kind: "at" },
    ]);
    expect(mirrorSegments("메일 kim@a", null)).toEqual([{ text: "메일 kim@a", kind: "text" }]);
  });
});
