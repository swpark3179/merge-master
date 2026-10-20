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

/** 마감 입력 칸의 조각: 날짜 `YYYY-MM-DD`, 시 `"00"`~`"23"`, 분 `"00"`~`"59"`. */
export interface DeadlineParts {
  date: string;
  hour: string;
  minute: string;
}

/** `"2026-10-10T18:00"` → 조각. 날짜만 있으면 그날 23:59(서버가 읽는 것과 같다). 형식이 틀리면 `null`. */
export function splitDeadline(text: string | undefined | null): DeadlineParts | null {
  if (!text) return null;
  const m = /^(\d{4}-\d{2}-\d{2})(?:T(\d{2}):(\d{2}))?/.exec(text.trim());
  if (!m || !parseDeadline(text)) return null;
  const [, date = "", hour = "23", minute = "59"] = m;
  return { date, hour, minute };
}

/** 조각 → `"YYYY-MM-DDTHH:MM"`. 하나라도 비었거나 틀렸으면 `null`(그때는 저장하지 않는다). */
export function joinDeadline(date: string, hour: string, minute: string): string | null {
  if (!/^\d{4}-\d{2}-\d{2}$/.test(date) || !/^\d{1,2}$/.test(hour) || !/^\d{1,2}$/.test(minute)) return null;
  const h = Number(hour);
  const mi = Number(minute);
  if (h > 23 || mi > 59) return null;
  const text = `${date}T${String(h).padStart(2, "0")}:${String(mi).padStart(2, "0")}`;
  const d = parseDeadline(text);
  // 2026-02-30처럼 없는 날은 Date가 다음 달로 넘기므로 되돌려 비교한다.
  if (!d || toLocalDate(d) !== date) return null;
  return text;
}

/** 분 고르기 칸의 선택지(`step`분 간격). 간격에 맞지 않는 지금 값(예: `"07"`)은 사라지지 않게 끼워 넣는다. */
export function minuteOptions(step: number, current?: string | null): string[] {
  const out: string[] = [];
  for (let m = 0; m < 60; m += Math.max(1, step)) out.push(String(m).padStart(2, "0"));
  if (current && /^\d{2}$/.test(current) && Number(current) < 60 && !out.includes(current)) {
    out.push(current);
    out.sort();
  }
  return out;
}

/** 이 기기 시각의 날짜 `YYYY-MM-DD`. */
export function toLocalDate(d: Date): string {
  const mo = String(d.getMonth() + 1).padStart(2, "0");
  const day = String(d.getDate()).padStart(2, "0");
  return `${d.getFullYear()}-${mo}-${day}`;
}

/** 이 기기 시각의 `YYYY-MM-DDTHH:MM`. */
export function toLocalDeadline(d: Date): string {
  const hh = String(d.getHours()).padStart(2, "0");
  const mm = String(d.getMinutes()).padStart(2, "0");
  return `${toLocalDate(d)}T${hh}:${mm}`;
}
