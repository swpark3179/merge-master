import type { CellRange } from "@mm/shared";
import { cellRefSpans, parseRange } from "@mm/shared";

/*
 * 채팅 입력 칸의 '@ 셀 선택' 규칙(순수 함수). 엑셀에서 수식을 칠 때 셀을 가리키는 참조 모드와 같은 감각이다.
 * 고른 범위는 기존 셀 참조 글자(`#시트!F6:H8`)로 들어가므로 서버·옛 글과 그대로 맞는다.
 */

/** 주소로 치는 중인 질의(`F`, `F6`, `F6:H`, `F6:H8`). */
const QUERY = /^[A-Za-z]{1,3}\d{0,7}(?::[A-Za-z]{0,3}\d{0,7})?$/;

/**
 * 방금 `@` 한 글자를 쳤고 그 자리가 글 맨 앞이거나 공백·줄바꿈 뒤면 `@`의 위치. 아니면 `null`(`kim@a1`처럼 붙여 쓴 @,
 * 붙여넣기, 여러 글자가 바뀐 경우).
 */
export function pickTrigger(prev: string, next: string, caret: number): number | null {
  if (next.length !== prev.length + 1 || caret < 1 || next.charAt(caret - 1) !== "@") return null;
  if (next.slice(0, caret - 1) + next.slice(caret) !== prev) return null;
  return caret === 1 || /\s/.test(next.charAt(caret - 2)) ? caret - 1 : null;
}

/** `@` 뒤에 친 질의(공백 앞까지). */
export function pickQuery(text: string, at: number): string {
  return /^\S*/.exec(text.slice(at + 1))![0];
}

/** 선택 모드에서 글이 바뀐 뒤: 계속(친 주소가 있으면 그 범위), 넣기(주소 뒤에 띄어쓰기), 끝(글자 @만 남김). */
export type PickInput =
  { kind: "keep"; range: CellRange | null } | { kind: "commit"; range: CellRange } | { kind: "exit" };

/**
 * `@`가 지워졌거나 주소가 아닌 글자를 쳤으면 끝. 질의 뒤에 띄어쓰기를 쳤을 때 질의가 주소면 넣고 아니면 끝.
 * 그 밖에는 계속하며 친 주소(시트 주소)를 미리 보인다.
 */
export function pickInput(text: string, at: number, caret: number): PickInput {
  if (text.charAt(at) !== "@") return { kind: "exit" };
  const q = pickQuery(text, at);
  if (q !== "" && !QUERY.test(q)) return { kind: "exit" };
  const range = q === "" ? null : parseRange(q);
  if (caret > at + 1 + q.length) return range ? { kind: "commit", range } : { kind: "exit" };
  return { kind: "keep", range };
}

/**
 * `@`와 질의를 참조 글자로 바꾼다. 뒤에 `extra`(보통 띄어쓰기 하나, 이미 띄어 있으면 겹치지 않게)를 붙이고 커서는 그
 * 뒤로. 글자 수 제한을 넘으면 `null`(넣지 않는다).
 */
export function insertToken(
  text: string,
  at: number,
  token: string,
  max: number,
  extra = " ",
): { text: string; caret: number } | null {
  const before = text.slice(0, at);
  const after = text.slice(at + 1 + pickQuery(text, at).length);
  const ins = token + (extra.startsWith(" ") && after.startsWith(" ") ? extra.slice(1) : extra);
  const out = before + ins + after;
  if (out.length > max) return null;
  return { text: out, caret: before.length + ins.length };
}

/**
 * 커서 바로 앞(Backspace)·바로 뒤(Delete)가 고르기로 넣은 참조면 통째로 지운다. 직접 친 참조(`tokens`에 없는 것)는
 * 한 글자씩 지운다. 지울 것이 없으면 `null`.
 */
export function deleteToken(
  text: string,
  caret: number,
  key: "Backspace" | "Delete",
  tokens: readonly string[],
): { text: string; caret: number } | null {
  for (const s of cellRefSpans(text)) {
    if (!tokens.includes(text.slice(s.start, s.end))) continue;
    if ((key === "Backspace" && s.end === caret) || (key === "Delete" && s.start === caret))
      return { text: text.slice(0, s.start) + text.slice(s.end), caret: s.start };
  }
  return null;
}

/** 입력 칸 뒤의 거울에 그릴 조각: 보통 글자, 셀 참조(칩 바탕), 선택 모드를 연 `@`. */
export interface Segment {
  text: string;
  kind: "text" | "ref" | "at";
}

export function mirrorSegments(text: string, at: number | null): Segment[] {
  const out: Segment[] = [];
  const plain = (from: number, to: number) => {
    if (to <= from) return;
    if (at !== null && at >= from && at < to) {
      if (at > from) out.push({ text: text.slice(from, at), kind: "text" });
      out.push({ text: "@", kind: "at" });
      if (at + 1 < to) out.push({ text: text.slice(at + 1, to), kind: "text" });
    } else out.push({ text: text.slice(from, to), kind: "text" });
  };
  let last = 0;
  for (const s of cellRefSpans(text)) {
    plain(last, s.start);
    out.push({ text: text.slice(s.start, s.end), kind: "ref" });
    last = s.end;
  }
  plain(last, text.length);
  return out;
}
