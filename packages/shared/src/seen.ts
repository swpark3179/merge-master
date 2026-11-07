import { formatDeadline, toLocalDeadline } from "./deadline";

/**
 * 마지막 접속 시각(유닉스 ms)을 이 기기 시각으로 짧게: 오늘이면 `"오늘 14:32"`, 어제면 `"어제 09:05"`, 올해면
 * `"11/5(목) 14:32"`, 그 전 해는 `"2025/11/5(수) 14:32"`.
 */
export function formatSeen(ms: number, now: Date = new Date()): string {
  const d = new Date(ms);
  const local = toLocalDeadline(d);
  const day = (x: Date) => new Date(x.getFullYear(), x.getMonth(), x.getDate()).getTime();
  const days = Math.round((day(now) - day(d)) / 86_400_000);
  if (days === 0) return `오늘 ${local.slice(11)}`;
  if (days === 1) return `어제 ${local.slice(11)}`;
  const short = formatDeadline(local);
  return d.getFullYear() === now.getFullYear() ? short : `${d.getFullYear()}/${short}`;
}

/** 마지막 접속 시각 전체(마우스를 올렸을 때): `"2026-11-07 14:32"`. */
export function formatSeenFull(ms: number): string {
  return toLocalDeadline(new Date(ms)).replace("T", " ");
}
