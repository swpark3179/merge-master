/** 지금 주소의 업무 경로(`/q4` → `q4`). 맨 위 주소(`/`)면 `null` — 업무 목록. */
export function currentSlug(): string | null {
  const first = location.pathname.split("/").filter(Boolean)[0];
  if (!first) return null;
  try {
    return decodeURIComponent(first);
  } catch {
    return first;
  }
}

/** 브라우저에 남기는 값의 이름 앞부분(업무 경로마다 따로). */
export function storageSlug(): string {
  return currentSlug() ?? "root";
}
