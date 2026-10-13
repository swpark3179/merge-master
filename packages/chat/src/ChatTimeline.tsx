import { motionOk } from "@mm/shared";
import type { ReactNode } from "react";
import { useEffect, useLayoutEffect, useRef, useState } from "react";

import type { TimelineItem } from "./timeline";
import { AT_BOTTOM_PX, firstUnread } from "./timeline";

/**
 * 채팅 목록: 열 때 안 읽은 첫 글 위에 '여기부터 새 메시지' 구분선(다시 열 때까지 그 자리), 바닥에 있거나 내가 보낸 글이면
 * 새 글을 따라 내려가고, 위로 올려 읽는 중이면 대신 '새 메시지 n개' 버튼에 수를 쌓는다. 처음 그린 글은 가만히 두고 뒤에 온
 * 글만 나타나며 움직인다.
 */
export function ChatTimeline({
  items,
  seen,
  onSeen,
  empty,
}: {
  items: TimelineItem[];
  /** 탭을 열기 전에 본 마지막 글 번호. */
  seen: number;
  onSeen: (id: number) => void;
  /** 글이 없을 때. */
  empty?: ReactNode;
}) {
  const list = useRef<HTMLDivElement>(null);
  const divider = useRef<HTMLDivElement>(null);
  // 맨 처음 글부터 모두 새 글이면(처음 여는 사람) 구분선은 알려 주는 것이 없어 그리지 않는다.
  const [unreadFrom] = useState(() => {
    const id = firstUnread(items, seen);
    return id !== null && id !== items[0]?.id ? id : null;
  });
  /** 처음 그릴 때 이미 있던 글(이 번호까지는 나타나며 움직이지 않는다). */
  const [initialLast] = useState(() => items[items.length - 1]?.id ?? 0);
  const [below, setBelow] = useState(0);
  const atBottom = useRef(true);
  const last = items[items.length - 1];
  const lastId = last?.id ?? 0;
  const prevLast = useRef(lastId);

  const isAtBottom = () => {
    const el = list.current;
    return !el || el.scrollHeight - el.scrollTop - el.clientHeight < AT_BOTTOM_PX;
  };
  const toBottom = (smooth: boolean) => {
    const el = list.current;
    el?.scrollTo({ top: el.scrollHeight, behavior: smooth && motionOk() ? "smooth" : "auto" });
  };

  // 처음: 구분선이 위쪽 3분의 1쯤 오게, 없으면 맨 아래.
  useLayoutEffect(() => {
    const el = list.current;
    if (!el) return;
    const d = divider.current;
    if (d) {
      const y = d.getBoundingClientRect().top - el.getBoundingClientRect().top + el.scrollTop;
      el.scrollTop = Math.max(0, y - el.clientHeight / 3);
    } else el.scrollTop = el.scrollHeight;
    atBottom.current = isAtBottom();
    // 처음 한 번만.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // 새 글: 바닥이었거나 내 글이면 따라 내려가고, 아니면 버튼에 센다.
  useEffect(() => {
    onSeen(lastId);
    const from = prevLast.current;
    prevLast.current = lastId;
    if (lastId <= from) return;
    const fresh = items.filter((it) => it.id > from);
    if (atBottom.current || fresh.some((it) => it.mine)) {
      toBottom(true);
      setBelow(0);
    } else setBelow((n) => n + fresh.filter((it) => it.counts ?? !it.mine).length);
    // 마지막 글 번호가 바뀔 때만.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [lastId, onSeen]);

  const onScroll = () => {
    atBottom.current = isAtBottom();
    if (atBottom.current && below > 0) setBelow(0);
  };

  return (
    <div className="chat-timeline">
      <div className="chat-list" ref={list} aria-label="채팅 내용" onScroll={onScroll}>
        {items.length === 0 && empty}
        {items.map((it) => (
          <div key={it.id} className={`chat-row${it.id > initialLast ? (it.mine ? " fresh mine" : " fresh") : ""}`}>
            {it.id === unreadFrom && (
              <div className="chat-divider" ref={divider}>
                <span>여기부터 새 메시지</span>
              </div>
            )}
            {it.node}
          </div>
        ))}
      </div>
      {below > 0 && (
        <div className="chat-jump-wrap">
          <button
            type="button"
            className="chat-jump"
            onClick={() => {
              toBottom(true);
              setBelow(0);
            }}
          >
            <svg
              width="12"
              height="12"
              viewBox="0 0 12 12"
              fill="none"
              stroke="currentColor"
              strokeWidth="1.6"
              strokeLinecap="round"
              strokeLinejoin="round"
              aria-hidden="true"
            >
              <path d="M6 2v8" />
              <path d="M2.5 6.5L6 10l3.5-3.5" />
            </svg>
            새 메시지 {below}개
          </button>
        </div>
      )}
    </div>
  );
}

/**
 * 채팅 한 통: 남의 글은 왼쪽에 사람 색 이니셜, 내 글은 오른쪽. 머리말(이름·받는 사람·시각)은 쓰는 쪽이 정한다.
 * 리액션 고르는 막대는 말풍선 오른쪽 위에 뜬다.
 */
export function ChatBubble({
  mine,
  name,
  color,
  head,
  children,
  reactions,
}: {
  mine: boolean;
  /** 이니셜에 쓸 이름. */
  name: string;
  /** 사람 색(이니셜 바탕). */
  color: string;
  head: ReactNode;
  children: ReactNode;
  reactions?: ReactNode;
}) {
  return (
    <div className={`chat-msg${mine ? " mine" : ""}`}>
      {!mine && (
        <span className="chat-avatar" style={{ background: color }} aria-hidden="true">
          {name.charAt(0)}
        </span>
      )}
      <div className="chat-bubble">
        <div className="chat-head">{head}</div>
        <div className="chat-text">{children}</div>
        {reactions}
      </div>
    </div>
  );
}
