import type { NoticeView } from "@mm/shared";
import { useState } from "react";

function hhmm(ms: number): string {
  const d = new Date(ms);
  return [d.getHours(), d.getMinutes()].map((n) => String(n).padStart(2, "0")).join(":");
}

/** 확성기 그림(공지 막대 왼쪽). */
function Megaphone() {
  return (
    <svg
      className="notice-icon"
      width="18"
      height="18"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="2"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden
    >
      <path d="M3 11v2a1 1 0 0 0 1 1h2l5 4V6L6 10H4a1 1 0 0 0-1 1z" />
      <path d="M15.5 8.5a5 5 0 0 1 0 7" />
      <path d="M18.5 5.5a9 9 0 0 1 0 13" />
    </svg>
  );
}

/**
 * 확인하지 않은 공지: 작업 화면 위에 고정한다(새로 고쳐도 확인할 때까지 남는다). '중요'는 빨간 막대, 일반은 파란 막대.
 * 화면을 연 뒤에 온 공지는 내려오며 테두리가 세 번 차오르고 확성기가 흔들린다. 둘 이상이면 '모두 확인'.
 * 작업은 막지 않는다(막대 아래 그리드는 그대로 쓸 수 있다).
 */
export function NoticeBanners({ notices, onAck }: { notices: NoticeView[]; onAck: (id: number) => void }) {
  // 처음 그릴 때 이미 있던 공지(이 번호까지는 움직이지 않는다).
  const [initialLast] = useState(() => notices.reduce((m, n) => Math.max(m, n.id), 0));
  const unread = notices
    .filter((n) => !n.read)
    .sort((a, b) => Number(b.important) - Number(a.important) || b.id - a.id);
  if (unread.length === 0) return null;
  return (
    <div className="notice-stack" aria-label="확인하지 않은 공지">
      {unread.length > 1 && (
        <div className="notice-stack-head">
          <span>
            확인하지 않은 공지 <b>{unread.length}</b>건
          </span>
          <button type="button" className="link" onClick={() => unread.forEach((n) => onAck(n.id))}>
            모두 확인
          </button>
        </div>
      )}
      {unread.map((n) => (
        <div
          key={n.id}
          className={`${n.important ? "important-bar" : "notice-bar"}${n.id > initialLast ? " fresh" : ""}`}
          role={n.important ? "alert" : "status"}
        >
          <Megaphone />
          <div className="notice-main">
            <div className="notice-meta">
              <b>{n.important ? "중요 공지" : "공지"}</b>
              <span>
                {n.from} · {hhmm(n.atMs)}
              </span>
            </div>
            <span className="grow">{n.text}</span>
          </div>
          <button type="button" className="notice-ack" onClick={() => onAck(n.id)}>
            확인
          </button>
        </div>
      ))}
    </div>
  );
}
