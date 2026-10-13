const DAYS = ["일", "월", "화", "수", "목", "금", "토"];

/** `"2026-10-10T18:00"`(로컬 시각) → Date. 형식이 틀리면 `null`. */
export function parseDeadline(text: string | undefined | null): Date | null {
  if (!text) return null;
  const m = /^(\d{4})-(\d{2})-(\d{2})(?:T(\d{2}):(\d{2}))?/.exec(text.trim());
  if (!m) return null;
  const [, y, mo, d, h = "23", mi = "59"] = m;
  const date = new Date(Number(y), Number(mo) - 1, Number(d), Number(h), Number(mi));
  return Number.isNaN(date.getTime()) ? null : date;
}

/** `"10/10(금) 18:00"`. */
export function formatDeadline(text: string | undefined | null): string {
  const d = parseDeadline(text);
  if (!d) return "";
  const hh = String(d.getHours()).padStart(2, "0");
  const mm = String(d.getMinutes()).padStart(2, "0");
  return `${d.getMonth() + 1}/${d.getDate()}(${DAYS[d.getDay()]}) ${hh}:${mm}`;
}

/** 마감까지 남은 날: `"D-2"`, 당일 `"D-day"`, 지났으면 `"마감"`. */
export function dday(text: string | undefined | null, now: Date = new Date()): string {
  const d = parseDeadline(text);
  if (!d) return "";
  if (d.getTime() <= now.getTime()) return "마감";
  const day = (x: Date) => new Date(x.getFullYear(), x.getMonth(), x.getDate()).getTime();
  const days = Math.round((day(d) - day(now)) / 86_400_000);
  return days <= 0 ? "D-day" : `D-${days}`;
}

/**
 * 마감까지 남은 시간(유닉스 ms끼리): `"2일 3시간"`, `"3시간 20분"`, `"12분"`, 1분 안쪽은 `"40초"`, 마감 시각부터
 * `"마감 지남"`. 분은 올려 센다(1분 30초 남았으면 `"2분"` — 서버가 막기 전에 '마감 지남'이 먼저 보이지 않게).
 * 마감이 없으면 빈 글자.
 */
export function remainingAt(dueMs: number | null | undefined, nowMs: number): string {
  if (dueMs == null || !Number.isFinite(dueMs)) return "";
  const left = dueMs - nowMs;
  if (left <= 0) return "마감 지남";
  if (left < 60_000) return `${Math.ceil(left / 1000)}초`;
  const mins = Math.ceil(left / 60_000);
  const days = Math.floor(mins / 1440);
  const hours = Math.floor((mins % 1440) / 60);
  if (days > 0) return hours > 0 ? `${days}일 ${hours}시간` : `${days}일`;
  if (hours > 0) return mins % 60 > 0 ? `${hours}시간 ${mins % 60}분` : `${hours}시간`;
  return `${mins}분`;
}

/** [`remainingAt`]과 같지만 마감 글자(`YYYY-MM-DDTHH:MM`, 이 기기의 시각으로 읽는다)로. 호스트 화면이 쓴다. */
export function remaining(text: string | undefined | null, now: Date = new Date()): string {
  return remainingAt(parseDeadline(text)?.getTime(), now.getTime());
}

/** 남은 시간 표시를 다시 그릴 간격(ms): 2분 안쪽은 1초마다, 그 밖은 15초마다. */
export function remainingTick(dueMs: number | null | undefined, nowMs: number): number {
  return dueMs != null && dueMs - nowMs < 120_000 ? 1_000 : 15_000;
}
