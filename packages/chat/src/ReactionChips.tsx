import type { ChatReaction, ChatReactionMark, ReactionGroup, ReactionViewer } from "@mm/shared";
import { CHAT_REACTIONS, reactionGroups } from "@mm/shared";
import { useEffect, useId, useRef } from "react";

import { Popover } from "./Popover";
import { useHoverIntent } from "./useHoverIntent";

/** 사람 색(명단 색). 호스트는 `host`로 가린다(이름이 바뀌어도 같은 사람). */
export type ColorOf = (name: string, host: boolean) => string;

export interface ReactionChipsProps {
  marks: ChatReactionMark[] | undefined;
  viewer: ReactionViewer;
  /** 남의 글이면 칩을 눌러 내 것을 달거나 떼고, 마우스를 올리면 고르는 막대가 나온다. 내 글이면 보기만. */
  canReact: boolean;
  onToggle: (kind: ChatReaction) => void;
  colorOf: ColorOf;
}

/**
 * 채팅 한 통의 리액션: 단 것은 칩(이모지·수). 칩에 머물거나 초점을 주면(터치는 길게 누르면) 단 사람 목록이 뜬다.
 * 처음 그린 뒤에 생긴 칩은 튀어나오고, 바뀐 수는 아래에서 올라온다.
 */
export function ReactionChips({ marks, viewer, canReact, onToggle, colorOf }: ReactionChipsProps) {
  const groups = reactionGroups(marks, viewer);
  // 앞서 그린 종류별 수(처음 그릴 때는 없음: 그때는 움직이지 않는다).
  const before = useRef<Map<ChatReaction, number> | null>(null);
  const prev = before.current;
  useEffect(() => {
    before.current = new Map(groups.map((g) => [g.kind, g.count]));
  });
  return (
    <>
      {groups.length > 0 && (
        <div className="chat-reactions">
          {groups.map((g) => {
            const had = prev?.get(g.kind);
            return (
              <ReactionChip
                key={g.kind}
                group={g}
                canReact={canReact}
                onToggle={onToggle}
                colorOf={colorOf}
                pop={prev !== null && had === undefined}
                bump={had !== undefined && had !== g.count}
              />
            );
          })}
        </div>
      )}
      {canReact && (
        <div className="reaction-bar" role="group" aria-label="리액션 달기">
          {CHAT_REACTIONS.map((r) => {
            const on = groups.some((g) => g.kind === r.kind && g.mine);
            return (
              <button
                key={r.kind}
                type="button"
                className={on ? "on" : undefined}
                aria-label={r.label}
                aria-pressed={on}
                title={r.label}
                onClick={() => onToggle(r.kind)}
              >
                {r.emoji}
              </button>
            );
          })}
        </div>
      )}
    </>
  );
}

function ReactionChip({
  group: g,
  canReact,
  onToggle,
  colorOf,
  pop,
  bump,
}: {
  group: ReactionGroup;
  canReact: boolean;
  onToggle: (kind: ChatReaction) => void;
  colorOf: ColorOf;
  pop: boolean;
  bump: boolean;
}) {
  const hover = useHoverIntent();
  const id = useId();
  const label = `${g.label} ${g.count}`;
  const cls = `reaction${g.mine ? " on" : ""}${pop ? " pop" : ""}`;
  const describedBy = hover.anchor ? id : undefined;
  const body = (
    <>
      <span aria-hidden="true">{g.emoji}</span>
      <span key={g.count} className={bump ? "count bump" : "count"}>
        {g.count}
      </span>
    </>
  );
  return (
    <>
      {canReact ? (
        <button
          type="button"
          className={cls}
          aria-label={label}
          aria-pressed={g.mine}
          aria-describedby={describedBy}
          {...hover.handlers}
          onClick={() => {
            if (!hover.consumeLongPress()) onToggle(g.kind);
          }}
        >
          {body}
        </button>
      ) : (
        // 내 글: 누를 수 없지만 초점을 받아 누가 달았는지 볼 수 있다.
        <span
          className={cls}
          role="img"
          aria-label={label}
          aria-describedby={describedBy}
          tabIndex={0}
          {...hover.handlers}
        >
          {body}
        </span>
      )}
      {hover.anchor && (
        <Popover anchor={hover.anchor} id={id} className="chat-who">
          <WhoReacted group={g} canReact={canReact} colorOf={colorOf} />
        </Popover>
      )}
    </>
  );
}

/** 단 사람 목록(단 순서): 사람 색 이니셜, '호스트'·'나' 표시, 남의 글이면 누르면 무엇이 되는지. */
function WhoReacted({ group: g, canReact, colorOf }: { group: ReactionGroup; canReact: boolean; colorOf: ColorOf }) {
  return (
    <>
      <div className="chat-who-head">
        <span className="emoji" aria-hidden="true">
          {g.emoji}
        </span>
        <b>{g.label}</b>
        <span>· {g.count}명</span>
      </div>
      <ul className="chat-who-list">
        {g.people.map((p, i) => (
          <li key={i}>
            <span className="chat-avatar sm" style={{ background: colorOf(p.name, p.host) }} aria-hidden="true">
              {p.name.charAt(0)}
            </span>
            <span className="name">{p.name}</span>
            {p.host && <span className="chat-badge">호스트</span>}
            {p.me && <span className="chat-badge me">나</span>}
          </li>
        ))}
      </ul>
      {canReact && <div className="chat-who-hint">{g.mine ? "누르면 내 리액션을 뗍니다" : "누르면 나도 답니다"}</div>}
    </>
  );
}
