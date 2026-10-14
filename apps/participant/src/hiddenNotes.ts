import { storageSlug } from "./slug";

/**
 * 내가 숨긴 강조 메모(말풍선): 이 브라우저에만, 주소 경로마다 기억한다. 강조 번호는 업무마다 1부터 다시 시작하므로
 * 업무 id와 함께 남기고, 다른 업무의 것이면 버린다. 노란 강조와 호스트의 확인 집계는 그대로다.
 */
interface Stored {
  job: string;
  ids: number[];
}

/** 남긴 글을 읽는다(다른 업무·깨진 글이면 빈 집합). */
export function parseHidden(raw: string | null, job: string): Set<number> {
  if (!raw) return new Set();
  try {
    const v = JSON.parse(raw) as Partial<Stored>;
    if (v.job !== job || !Array.isArray(v.ids)) return new Set();
    return new Set(v.ids.filter((x): x is number => Number.isInteger(x)));
  } catch {
    return new Set();
  }
}

export function serializeHidden(job: string, ids: Set<number>): string {
  return JSON.stringify({ job, ids: [...ids].sort((a, b) => a - b) } satisfies Stored);
}

/** 지금 있는 강조의 것만 남긴다(호스트가 해제한 강조는 잊는다). 바뀐 것이 없으면 같은 집합을 돌려준다. */
export function pruneHidden(ids: Set<number>, alive: Iterable<number>): Set<number> {
  const keep = new Set(alive);
  if ([...ids].every((id) => keep.has(id))) return ids;
  return new Set([...ids].filter((id) => keep.has(id)));
}

/** 하나를 숨기거나 다시 보인다. */
export function toggleHidden(ids: Set<number>, id: number): Set<number> {
  const next = new Set(ids);
  if (!next.delete(id)) next.add(id);
  return next;
}

function key(): string {
  return `mm.${storageSlug()}.notes.hidden`;
}

export function loadHiddenNotes(job: string): Set<number> {
  try {
    return parseHidden(localStorage.getItem(key()), job);
  } catch {
    return new Set();
  }
}

export function saveHiddenNotes(job: string, ids: Set<number>) {
  try {
    if (ids.size === 0) localStorage.removeItem(key());
    else localStorage.setItem(key(), serializeHidden(job, ids));
  } catch {
    // 사생활 보호 모드 등: 기억하지 못해도 이번 화면에서는 숨긴다.
  }
}
