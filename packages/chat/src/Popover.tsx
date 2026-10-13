import type { ReactNode } from "react";
import { useLayoutEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";

import type { Placement } from "./place";
import { placePopover } from "./place";

/**
 * 칩에 붙는 작은 카드(누가 리액션했나, 셀 참조 미리보기). 사이드바·그리드 띠보다 위에 그리도록 body에 붙이고, 기준이
 * 스크롤로 움직이거나 내용 크기가 바뀌면 자리를 다시 잡는다. 마우스는 밑으로 넘긴다(툴팁).
 */
export function Popover({
  anchor,
  id,
  className,
  children,
}: {
  anchor: HTMLElement;
  /** 기준의 `aria-describedby`가 가리킬 id. */
  id: string;
  className?: string;
  children: ReactNode;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const [pos, setPos] = useState<Placement | null>(null);

  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const place = () => {
      const a = anchor.getBoundingClientRect();
      // 나타나는 동안 작게 시작하므로(scale) 겉보기 크기가 아니라 배치 크기로 잰다.
      const size = { width: el.offsetWidth, height: el.offsetHeight };
      setPos(placePopover(a, size, { width: window.innerWidth, height: window.innerHeight }));
    };
    place();
    const ro = new ResizeObserver(place);
    ro.observe(el);
    window.addEventListener("resize", place);
    window.addEventListener("scroll", place, true);
    return () => {
      ro.disconnect();
      window.removeEventListener("resize", place);
      window.removeEventListener("scroll", place, true);
    };
  }, [anchor]);

  return createPortal(
    <div
      ref={ref}
      id={id}
      role="tooltip"
      className={`chat-pop${pos ? ` ${pos.side}` : ""}${className ? ` ${className}` : ""}`}
      style={pos ? { left: pos.left, top: pos.top } : { left: 0, top: 0, visibility: "hidden" }}
    >
      {children}
    </div>,
    document.body,
  );
}
