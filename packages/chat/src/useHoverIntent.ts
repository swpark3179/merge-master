import type { FocusEvent, KeyboardEvent, MouseEvent, TouchEvent } from "react";
import { useCallback, useEffect, useRef, useState } from "react";

/** 마우스가 머물면 여는 시간·떠나면 닫는 시간·터치로 길게 누르는 시간(ms). */
export const HOVER_OPEN_MS = 350;
export const HOVER_CLOSE_MS = 120;
export const LONG_PRESS_MS = 450;

/**
 * 칩 위 팝오버를 열고 닫는 때: 마우스는 잠깐 머문 뒤(지나가기만 하면 열지 않는다), 키보드 초점은 바로, 터치는 길게 눌러.
 * 닫히기 전에 마우스가 돌아오면 그대로 둔다. Esc로 닫는다.
 */
export function useHoverIntent() {
  const [anchor, setAnchor] = useState<HTMLElement | null>(null);
  const timer = useRef<number | undefined>(undefined);
  /** 길게 눌러 열었다: 손을 뗄 때 오는 click은 누른 것으로 치지 않는다. */
  const pressed = useRef(false);
  const open = useRef(false);
  /** 마우스·터치로 눌러 초점이 왔다. */
  const byPointer = useRef(false);
  open.current = anchor !== null;

  const clear = () => window.clearTimeout(timer.current);
  useEffect(() => clear, []);
  const later = (fn: () => void, ms: number) => {
    clear();
    timer.current = window.setTimeout(fn, ms);
  };
  const close = useCallback(() => {
    clear();
    setAnchor(null);
  }, []);

  const handlers = {
    onMouseEnter: (e: MouseEvent<HTMLElement>) => {
      const el = e.currentTarget;
      if (open.current) {
        clear();
        setAnchor(el);
      } else later(() => setAnchor(el), HOVER_OPEN_MS);
    },
    onMouseLeave: () => later(() => setAnchor(null), HOVER_CLOSE_MS),
    onPointerDown: () => {
      byPointer.current = true;
    },
    onFocus: (e: FocusEvent<HTMLElement>) => {
      // 눌러서 생긴 초점은 머무름이 이미 다룬다(누르자마자 열리지 않게).
      const pointer = byPointer.current;
      byPointer.current = false;
      if (pointer) return;
      clear();
      setAnchor(e.currentTarget);
    },
    onBlur: close,
    onTouchStart: (e: TouchEvent<HTMLElement>) => {
      const el = e.currentTarget;
      pressed.current = false;
      later(() => {
        pressed.current = true;
        setAnchor(el);
      }, LONG_PRESS_MS);
    },
    onTouchMove: () => clear(),
    onTouchEnd: () => {
      if (!pressed.current) clear();
    },
    onKeyDown: (e: KeyboardEvent<HTMLElement>) => {
      if (e.key !== "Escape" || !open.current) return;
      e.stopPropagation();
      close();
    },
  };

  /** 이번 click이 길게 누르기 끝이었으면 `true`(그 click은 무시한다). */
  const consumeLongPress = () => {
    const was = pressed.current;
    pressed.current = false;
    return was;
  };

  return { anchor, handlers, close, show: (el: HTMLElement) => (clear(), setAnchor(el)), consumeLongPress };
}
