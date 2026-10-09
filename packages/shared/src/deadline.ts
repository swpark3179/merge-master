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

/** 마감까지 남은 시간: `"2일 3시간"`, `"3시간 20분"`, `"12분"`, 지났으면 `"마감 지남"`. 마감이 없으면 빈 글자. */
export function remaining(text: string | undefined | null, now: Date = new Date()): string {
  const d = parseDeadline(text);
  if (!d) return "";
  const mins = Math.floor((d.getTime() - now.getTime()) / 60_000);
  if (mins <= 0) return "마감 지남";
  const days = Math.floor(mins / 1440);
  const hours = Math.floor((mins % 1440) / 60);
  if (days > 0) return hours > 0 ? `${days}일 ${hours}시간` : `${days}일`;
  if (hours > 0) return `${hours}시간 ${mins % 60}분`;
  return `${mins}분`;
}
