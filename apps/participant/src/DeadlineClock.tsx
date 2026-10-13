import { formatDeadline, parseDeadline, remainingAt, remainingTick } from "@mm/shared";
import { useEffect, useState } from "react";

/**
 * 머리줄 오른쪽 위의 마감 시각과 남은 시간. 시각은 호스트 PC 기준이다: 마감 시각은 호스트가 정한 글자 그대로 보이고,
 * 남은 시간은 호스트가 계산한 마감 시각(`dueAt`)과 호스트 시계(`이 기기 시각 + offset`)로 센다. 2분 안쪽은 1초마다,
 * 그 밖은 15초마다 다시 그린다(이 부분만 다시 그린다).
 */
export function DeadlineClock({
  deadline,
  dueAt,
  offset,
}: {
  /** 호스트 현지 시각 `YYYY-MM-DDTHH:MM`. */
  deadline: string | undefined;
  /** 호스트가 계산한 마감 시각(유닉스 ms). 옛 서버면 없다(그때는 이 기기 시각으로 읽는다). */
  dueAt: number | undefined;
  /** 호스트 PC 시계 − 이 기기 시계(ms). */
  offset: number;
}) {
  const due = dueAt ?? parseDeadline(deadline)?.getTime();
  const [now, setNow] = useState(() => Date.now() + offset);
  useEffect(() => {
    if (due === undefined) return;
    let timer: number;
    const tick = () => {
      const t = Date.now() + offset;
      setNow(t);
      timer = window.setTimeout(tick, remainingTick(due, t));
    };
    tick();
    return () => window.clearTimeout(timer);
  }, [due, offset]);
  if (!deadline || due === undefined) return null;
  const left = remainingAt(due, now);
  const overdue = left === "마감 지남";
  const urgent = !overdue && due - now < 3_600_000;
  return (
    <div
      className={`deadline-left${overdue || urgent ? " urgent" : ""}`}
      role="timer"
      aria-label={overdue ? "마감 지남" : `마감까지 ${left} 남음`}
      title="호스트 PC 시각 기준"
    >
      <div className="eyebrow">마감 {formatDeadline(deadline)}</div>
      <b>{overdue ? left : `${left} 남음`}</b>
    </div>
  );
}
