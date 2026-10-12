/** 다른 사람의 편집이 도착한 칸을 물들이는 시간(ms). tokens.css의 --dur-flash와 같아야 한다(motion.test.ts). */
export const FLASH_MS = 1200;

/** 동작을 보여도 되는지('동작 줄이기'를 켜지 않았는지). JS로 움직이는 것(부드러운 스크롤 등)이 따른다. */
export function motionOk(): boolean {
  return typeof window === "undefined" || !window.matchMedia?.("(prefers-reduced-motion: reduce)").matches;
}

/** 새 항목으로 스크롤할 때: 처음에는 바로, 그 뒤로는(동작을 보여도 되면) 부드럽게. */
export function scrollBehavior(first: boolean): ScrollBehavior {
  return !first && motionOk() ? "smooth" : "auto";
}
