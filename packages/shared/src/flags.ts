/**
 * 셀 상태 비트. Rust `collect_core::access::flag`와 같은 값이어야 한다(바꾸면 양쪽을 함께 바꾼다).
 */
export const FLAG = {
  /** 입력 영역(배정된 사람이 입력할 수 있는 셀). */
  INPUT: 1,
  REQUIRED: 2,
  /** 셀 예외가 걸린 셀. */
  EXCEPTION: 4,
  FORMULA: 8,
  HIDDEN: 16,
  HEADER: 32,
  /** 병합 영역에서 왼쪽 위가 아닌 셀. */
  COVERED: 64,
  /** 보는 사람이 입력할 수 있다. */
  MINE: 128,
} as const;

/** `ConfigView.flags`(base64, 셀당 1바이트) → 바이트 배열. */
export function decodeFlags(b64: string): Uint8Array {
  const bin = atob(b64);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}

/** 담당자 색(명단 순서대로). 와이어프레임의 이름표 색과 같다. */
export const PERSON_COLORS = [
  "#2f9e6b",
  "#e07b28",
  "#8a5cd6",
  "#c2417a",
  "#2a78d6",
  "#a86a00",
  "#1f8a8a",
  "#7a5c3a",
  "#d64545",
  "#5a6b7d",
] as const;

export function personColor(index: number): string {
  return PERSON_COLORS[((index % PERSON_COLORS.length) + PERSON_COLORS.length) % PERSON_COLORS.length]!;
}
