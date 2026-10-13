import type { KeyboardEvent, ReactNode } from "react";
import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";

/**
 * 오른쪽 클릭 메뉴: 마우스 위치에 띄우고 화면 밖으로 넘치면 안으로 당긴다. Esc·바깥 클릭·창 크기 바꾸기·다른 곳으로
 * 포커스가 가면 닫힌다. ↑↓로 항목을 옮긴다. 그리드 띠(z 2~4)·편집기(6) 위에 그리도록 body에 붙인다.
 */
export interface ContextMenuProps {
  x: number;
  y: number;
  onClose: () => void;
  /** 메뉴 이름(화면 낭독기). */
  label: string;
  children: ReactNode;
}

const EDGE = 8;

export function ContextMenu({ x, y, onClose, label, children }: ContextMenuProps) {
  const ref = useRef<HTMLDivElement>(null);
  const [pos, setPos] = useState({ left: x, top: y });

  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const r = el.getBoundingClientRect();
    setPos({
      left: Math.max(EDGE, Math.min(x, window.innerWidth - r.width - EDGE)),
      top: Math.max(EDGE, Math.min(y, window.innerHeight - r.height - EDGE)),
    });
    // 처음 항목에 포커스(키보드로 바로 고를 수 있게). 숫자 칸이 먼저면 그 칸.
    el.querySelector<HTMLElement>("[data-menu-focus]:not(:disabled), .mmg-menu-item:not(:disabled)")?.focus();
  }, [x, y]);

  const close = useRef(onClose);
  close.current = onClose;
  useEffect(() => {
    const outside = (e: MouseEvent) => {
      if (!ref.current?.contains(e.target as Node)) close.current();
    };
    const away = () => close.current();
    document.addEventListener("mousedown", outside, true);
    document.addEventListener("contextmenu", outside, true);
    window.addEventListener("resize", away);
    window.addEventListener("blur", away);
    return () => {
      document.removeEventListener("mousedown", outside, true);
      document.removeEventListener("contextmenu", outside, true);
      window.removeEventListener("resize", away);
      window.removeEventListener("blur", away);
    };
  }, []);

  const onKeyDown = (e: KeyboardEvent) => {
    e.stopPropagation(); // 그리드가 방향키·글자를 받지 않게
    if (e.key === "Escape") {
      e.preventDefault();
      onClose();
      return;
    }
    if (e.key !== "ArrowDown" && e.key !== "ArrowUp") return;
    if ((e.target as HTMLElement).matches("input")) return; // 숫자 칸·슬라이더는 제 방향키를 쓴다
    const items = [...(ref.current?.querySelectorAll<HTMLElement>(".mmg-menu-item:not(:disabled)") ?? [])];
    if (items.length === 0) return;
    e.preventDefault();
    const i = items.indexOf(document.activeElement as HTMLElement);
    const next = e.key === "ArrowDown" ? (i + 1) % items.length : (i - 1 + items.length) % items.length;
    items[next]!.focus();
  };

  return createPortal(
    <div
      ref={ref}
      className="mmg-menu"
      role="menu"
      aria-label={label}
      style={pos}
      onKeyDown={onKeyDown}
      onContextMenu={(e) => e.preventDefault()}
    >
      {children}
    </div>,
    document.body,
  );
}

export interface MenuItemProps {
  onClick: () => void;
  disabled?: boolean;
  /** 비활성일 때 까닭(마우스를 올리면 보인다). */
  title?: string;
  danger?: boolean;
  children: ReactNode;
}

export function MenuItem({ onClick, disabled, title, danger, children }: MenuItemProps) {
  return (
    <button
      type="button"
      role="menuitem"
      className={`mmg-menu-item${danger ? " danger" : ""}`}
      disabled={disabled}
      title={title}
      onClick={onClick}
    >
      {children}
    </button>
  );
}

export function MenuSep() {
  return <div className="mmg-menu-sep" role="separator" />;
}

/** 메뉴 안 작은 제목·안내 글. */
export function MenuNote({ children, title }: { children: ReactNode; title?: boolean }) {
  return <div className={title ? "mmg-menu-title" : "mmg-menu-note"}>{children}</div>;
}

/** 열 너비·행 높이 범위(pt). 슬라이더는 흔히 쓰는 범위만, 숫자 칸은 Excel이 받는 범위 전체. */
export const SIZE_LIMITS = {
  col: { min: 2, max: 1000, slider: [5, 400] as const },
  row: { min: 2, max: 409, slider: [5, 200] as const },
};

export interface SizeControlProps {
  axis: "col" | "row";
  /** 지금 크기(pt). 여러 줄이 다르면 첫 줄. */
  value: number;
  /** 줄 수(제목에 'N열'로 보인다). */
  count: number;
  /** 놓거나 Enter·칸 벗어남에 한 번 부른다(바뀌었을 때만). */
  onCommit: (pt: number) => void;
  disabled?: boolean;
}

/** 크기 조절: 볼륨처럼 끄는 슬라이더 + 값 입력(pt). 메뉴는 열어 둔 채 여러 번 고칠 수 있다. */
export function SizeControl({ axis, value, count, onCommit, disabled }: SizeControlProps) {
  const limits = SIZE_LIMITS[axis];
  const name = axis === "col" ? "열 너비" : "행 높이";
  const round = (n: number) => Math.round(n * 4) / 4;
  const [v, setV] = useState(round(value));
  const [text, setText] = useState(String(round(value)));
  const committed = useRef(round(value));
  useEffect(() => {
    committed.current = round(value);
    setV(round(value));
    setText(String(round(value)));
  }, [value]);

  const commit = (n: number) => {
    const pt = round(Math.min(limits.max, Math.max(limits.min, n)));
    setV(pt);
    setText(String(pt));
    if (Number.isFinite(pt) && pt !== committed.current) {
      committed.current = pt;
      onCommit(pt);
    }
  };

  return (
    <div className="mmg-size" role="group" aria-label={name}>
      <span className="mmg-size-label">
        {name}
        {count > 1 ? ` (${count}${axis === "col" ? "열" : "행"})` : ""}
      </span>
      <input
        type="number"
        aria-label={`${name}(pt)`}
        data-menu-focus
        min={limits.min}
        max={limits.max}
        step={0.25}
        value={text}
        disabled={disabled}
        onChange={(e) => setText(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter") {
            e.preventDefault();
            commit(Number(text));
          }
        }}
        onBlur={() => commit(Number(text))}
      />
      <span className="mmg-size-unit">pt</span>
      <input
        type="range"
        aria-label={`${name} 조절`}
        min={limits.slider[0]}
        max={limits.slider[1]}
        step={1}
        value={Math.min(limits.slider[1], Math.max(limits.slider[0], v))}
        disabled={disabled}
        onChange={(e) => {
          setV(Number(e.target.value));
          setText(e.target.value);
        }}
        onPointerUp={() => commit(v)}
        onKeyUp={(e) => {
          if (e.key.startsWith("Arrow") || e.key === "Home" || e.key === "End" || e.key.startsWith("Page")) commit(v);
        }}
      />
    </div>
  );
}
