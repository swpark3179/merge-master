import { useCallback, useEffect, useRef } from "react";

/** 알릴 것: 새로 온 남의 채팅 수와 새 공지. */
export interface Pending {
  chat: number;
  notice: boolean;
}

/** 탭 제목 앞에 붙일 글. 공지가 먼저, 없으면 새 채팅 수. 알릴 것이 없으면 `null`. */
export function alertLabel(p: Pending): string | null {
  if (p.notice) return p.chat > 0 ? `📢 새 공지 · 💬 ${p.chat}` : "📢 새 공지";
  if (p.chat > 0) return `💬 새 메시지 ${p.chat}개`;
  return null;
}

/** 깜빡이는 동안의 한 박자: 켜짐이면 알림 글 + 평소 제목, 꺼짐이면 평소 제목. */
export function blinkTitle(base: string, p: Pending, on: boolean): string {
  const label = alertLabel(p);
  return on && label ? `${label} · ${base}` : base;
}

/** 지금 이 탭을 보고 있다(보이고, 창에 초점이 있다). */
function attended(): boolean {
  return document.visibilityState === "visible" && document.hasFocus();
}

const BLINK_MS = 1000;

/**
 * 탭이 가려졌거나 다른 창을 보는 동안 온 채팅·공지를 탭 제목 깜빡임으로 알린다(호스트 작업 표시줄 깜빡임의 웹판:
 * 참여자 주소는 http라 OS 알림을 쓸 수 없다). 돌아오면(보임·초점) 멈추고 평소 제목(`base`)으로 되돌린다.
 * 보고 있을 때 온 것은 알리지 않는다(화면 안 토스트·배지가 알린다).
 */
export function useTitleAlert(base: string): (kind: "chat" | "notice") => void {
  const pending = useRef<Pending>({ chat: 0, notice: false });
  const timer = useRef<number | null>(null);
  const baseRef = useRef(base);
  baseRef.current = base;

  useEffect(() => {
    const stop = () => {
      if (timer.current === null) return;
      window.clearInterval(timer.current);
      timer.current = null;
      pending.current = { chat: 0, notice: false };
      document.title = baseRef.current;
    };
    const back = () => {
      if (attended()) stop();
    };
    window.addEventListener("focus", back);
    document.addEventListener("visibilitychange", back);
    return () => {
      window.removeEventListener("focus", back);
      document.removeEventListener("visibilitychange", back);
      stop();
    };
  }, []);

  return useCallback((kind: "chat" | "notice") => {
    if (attended()) return;
    if (kind === "chat") pending.current.chat++;
    else pending.current.notice = true;
    if (timer.current !== null) return; // 이미 깜빡이는 중: 다음 박자에 새 글이 보인다
    let on = false;
    const tick = () => {
      on = !on;
      document.title = blinkTitle(baseRef.current, pending.current, on);
    };
    tick();
    timer.current = window.setInterval(tick, BLINK_MS);
  }, []);
}
