import type { ChatReaction, ChatReactionMark } from "./generated";

/** 채팅 리액션(보이는 순서). */
export const CHAT_REACTIONS: readonly { kind: ChatReaction; emoji: string; label: string }[] = [
  { kind: "ok", emoji: "👌", label: "OK" },
  { kind: "yes", emoji: "✅", label: "네" },
  { kind: "no", emoji: "❌", label: "아니요" },
  { kind: "thanks", emoji: "🙏", label: "감사합니다" },
  { kind: "good", emoji: "👍", label: "좋아요" },
  { kind: "sad", emoji: "😢", label: "슬퍼요" },
];

/** 리액션을 보는 사람: 참여자는 이름, 호스트는 `host`(이름이 바뀌어도 같은 사람). */
export type ReactionViewer = { name: string } | { host: true };

export interface ReactionGroup {
  kind: ChatReaction;
  emoji: string;
  label: string;
  count: number;
  /** 단 사람(단 순서). */
  names: string[];
  /** 단 사람(단 순서): 호스트인지, 나인지. 누가 달았는지 목록에 쓴다. */
  people: ReactionPerson[];
  /** 내가 달았다. */
  mine: boolean;
}

export interface ReactionPerson {
  name: string;
  host: boolean;
  me: boolean;
}

/** 채팅 한 통의 리액션을 종류별로 모은다(종류 순서, 아무도 달지 않은 종류는 빼고). */
export function reactionGroups(marks: readonly ChatReactionMark[] | undefined, me: ReactionViewer): ReactionGroup[] {
  const isMe = (m: ChatReactionMark) => ("host" in me ? m.host : !m.host && m.by === me.name);
  return CHAT_REACTIONS.flatMap((r) => {
    const of = (marks ?? []).filter((m) => m.kind === r.kind);
    if (of.length === 0) return [];
    const people = of.map((m) => ({ name: m.by, host: m.host, me: isMe(m) }));
    return [{ ...r, count: of.length, names: of.map((m) => m.by), people, mine: people.some((p) => p.me) }];
  });
}

/**
 * 채팅 입력 칸의 Enter: 그냥 Enter는 보내고, Ctrl(⌘)·Alt+Enter는 줄을 바꾼다(`newline`, 직접 넣는다). Shift+Enter는
 * textarea가 스스로 줄을 바꾸고, 한글 조합 중 Enter는 조합 확정이라 둘 다 `null`(기본 동작).
 */
export function chatEnterKey(e: {
  key: string;
  shiftKey: boolean;
  ctrlKey: boolean;
  metaKey: boolean;
  altKey: boolean;
  keyCode: number;
  nativeEvent: { isComposing: boolean };
}): "send" | "newline" | null {
  if (e.key !== "Enter" || e.nativeEvent.isComposing || e.keyCode === 229 || e.shiftKey) return null;
  return e.ctrlKey || e.metaKey || e.altKey ? "newline" : "send";
}
