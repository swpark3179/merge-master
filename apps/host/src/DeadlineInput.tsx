import { joinDeadline, minuteOptions, splitDeadline } from "@mm/shared";
import type { ReactNode } from "react";
import { useEffect, useState } from "react";

/** 분 고르기 간격(분). */
const MINUTE_STEP = 5;
/** 날짜를 처음 고르면 채우는 시각. */
const DEFAULT_TIME = { hour: "18", minute: "00" };
const HOURS = Array.from({ length: 24 }, (_, h) => String(h).padStart(2, "0"));

interface Props {
  /** 지금 마감(`YYYY-MM-DDTHH:MM`, 없으면 `undefined`). */
  value: string | undefined;
  /** 날짜·시·분이 모두 정해졌을 때만(또는 '마감 지우기') 부른다. */
  onChange: (next: string | undefined) => void;
  /** 필수면 '마감 지우기'가 없고, 비어 있으면 빨간 테두리. */
  required?: boolean;
  /** 고를 수 있는 가장 이른 날짜(`YYYY-MM-DD`). */
  minDate?: string;
  disabled?: boolean;
  /** 마감을 잴 시계 안내(예: '호스트 PC 시각 기준 (지금 14:20)'). */
  clock?: ReactNode;
}

/**
 * 입력 마감: 날짜 칸 + 시·분 고르기. 시·분은 고르기만 하고(키보드로 칠 수 없음), 날짜 칸도 키 입력을 막고 달력으로만
 * 고른다. 화면 안에서 들고 있다가 셋이 모두 정해졌을 때만 저장한다 — 고치는 도중에 마감이 지워지지 않는다.
 */
export function DeadlineInput({ value, onChange, required = false, minDate, disabled, clock }: Props) {
  const [draft, setDraft] = useState(() => splitDeadline(value) ?? { date: "", hour: "", minute: "" });
  useEffect(() => {
    setDraft(splitDeadline(value) ?? { date: "", hour: "", minute: "" });
  }, [value]);

  const update = (patch: Partial<typeof draft>) => {
    let next = { ...draft, ...patch };
    if (patch.date && !next.hour && !next.minute) next = { ...next, ...DEFAULT_TIME };
    setDraft(next);
    const text = joinDeadline(next.date, next.hour, next.minute);
    if (text && text !== value) onChange(text);
  };

  const empty = !draft.date;
  const invalid = required && empty;
  return (
    <span className="deadline-input">
      <span className="deadline-parts">
        <input
          aria-label="마감 날짜"
          aria-required={required}
          aria-invalid={invalid}
          className={invalid ? "invalid" : undefined}
          type="date"
          value={draft.date}
          min={minDate}
          disabled={disabled}
          title="달력에서 날짜를 고르세요"
          onKeyDown={(e) => {
            // Tab(칸 이동)·Esc(달력 닫기)만 둔다: 날짜는 달력으로만 고른다.
            if (e.key !== "Tab" && e.key !== "Escape") e.preventDefault();
            if (e.key === "Enter" || e.key === " ") openPicker(e.currentTarget);
          }}
          onPaste={(e) => e.preventDefault()}
          onClick={(e) => openPicker(e.currentTarget)}
          onChange={(e) => e.target.value && update({ date: e.target.value })}
        />
        <select
          aria-label="마감 시"
          className={invalid ? "invalid" : undefined}
          value={draft.hour}
          disabled={disabled || empty}
          onChange={(e) => update({ hour: e.target.value })}
        >
          {!draft.hour && <option value="">시</option>}
          {HOURS.map((h) => (
            <option key={h} value={h}>
              {h}시
            </option>
          ))}
        </select>
        <select
          aria-label="마감 분"
          className={invalid ? "invalid" : undefined}
          value={draft.minute}
          disabled={disabled || empty}
          onChange={(e) => update({ minute: e.target.value })}
        >
          {!draft.minute && <option value="">분</option>}
          {minuteOptions(MINUTE_STEP, draft.minute).map((m) => (
            <option key={m} value={m}>
              {m}분
            </option>
          ))}
        </select>
        {!required && value && (
          <button type="button" className="ghost small" disabled={disabled} onClick={() => onChange(undefined)}>
            마감 지우기
          </button>
        )}
      </span>
      {clock}
    </span>
  );
}

/** 달력을 연다(지원하지 않는 웹뷰면 그냥 둔다 — 날짜 칸을 눌러 여는 기본 동작이 남는다). */
function openPicker(input: HTMLInputElement) {
  try {
    input.showPicker?.();
  } catch {
    // 사용자 동작이 아닐 때 등: 무시
  }
}
