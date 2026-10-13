import type { ReactNode } from "react";

/** 바닥에서 이만큼(px) 안이면 바닥에 있는 것으로 본다(새 글이 오면 따라 내려간다). */
export const AT_BOTTOM_PX = 48;

export interface TimelineItem {
  id: number;
  /** 내가 보낸 글(보내면 늘 맨 아래로 간다, 안 읽은 수에 세지 않는다). */
  mine: boolean;
  /** 안 읽은 수·구분선에 셀 글인지(호스트는 참여자 글만 센다). 없으면 내 글이 아닌 것. */
  counts?: boolean;
  node: ReactNode;
}

/** 탭을 열 때 본 마지막 글 다음의 첫 안 읽은 글(셀 글만). 없으면 `null`. */
export function firstUnread(
  items: readonly Pick<TimelineItem, "id" | "mine" | "counts">[],
  seen: number,
): number | null {
  for (const it of items) if (it.id > seen && (it.counts ?? !it.mine)) return it.id;
  return null;
}
