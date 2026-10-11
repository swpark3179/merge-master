import type { SessionListItem } from "@mm/shared";

/** 업무 목록 순서: 내 담당 먼저, 그다음 호스트가 공개한 순서. */
export function sortSessions(list: SessionListItem[]): SessionListItem[] {
  return list
    .map((item, i) => ({ item, i }))
    .sort((a, b) => Number(b.item.mine) - Number(a.item.mine) || a.i - b.i)
    .map((x) => x.item);
}

/** 업무 주소(경로 조각은 인코딩). */
export function sessionHref(slug: string): string {
  return `/${encodeURIComponent(slug)}`;
}

/** 카드에 붙일 상태 글자. */
export function sessionState(item: SessionListItem): string | null {
  if (item.closed) return "취합 마감됨";
  if (item.readOnly) return "마감 시각 지남";
  return null;
}
