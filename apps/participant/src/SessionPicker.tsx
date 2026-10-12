import type { SessionListItem } from "@mm/shared";
import { formatDeadline } from "@mm/shared";
import { useEffect, useState } from "react";

import { sessionHref, sessionState, sortSessions } from "./picker";

const POLL_MS = 5_000;

/**
 * 맨 위 주소(`http://호스트IP:포트/`): 호스트가 공개한 취합 업무 목록. 업무를 고르면 그 업무 주소로 간다. 같은 PC에서 여러 업무를
 * 동시에 할 때는 업무마다 탭으로 열면 된다(업무마다 이름·대기열을 따로 기억한다).
 */
export function SessionPicker() {
  const [list, setList] = useState<SessionListItem[] | null>(null);
  const [error, setError] = useState(false);

  useEffect(() => {
    document.title = "취합 업무 · 엑셀 취합";
    let stop = false;
    const get = () =>
      fetch("/api/sessions")
        .then((r) => (r.ok ? (r.json() as Promise<SessionListItem[]>) : Promise.reject(new Error(String(r.status)))))
        .then((l) => {
          if (stop) return;
          setList(l);
          setError(false);
        })
        .catch(() => !stop && setError(true));
    void get();
    const id = window.setInterval(get, POLL_MS);
    return () => {
      stop = true;
      window.clearInterval(id);
    };
  }, []);

  const sorted = list ? sortSessions(list) : [];
  return (
    <div className="picker-wrap">
      <div className="picker">
        <div className="muted">엑셀 취합</div>
        <h1>취합 업무</h1>
        <p className="muted">참여할 업무를 고르세요. 여러 업무를 함께 할 때는 업무마다 새 탭으로 열면 됩니다.</p>
        {error && (
          <div className="state-card" role="alert">
            <b>호스트 PC에 연결할 수 없습니다</b>
            <div className="muted">같은 사내망인지 확인해 주세요. 잠시 뒤 다시 시도합니다.</div>
          </div>
        )}
        {list && sorted.length === 0 && (
          <div className="state-card" role="status">
            <b>지금 열린 취합 업무가 없습니다</b>
            <div className="muted">호스트가 업무를 공개하면 여기에 보입니다.</div>
          </div>
        )}
        <ul className="picker-list" aria-label="취합 업무 목록" aria-busy={list === null && !error}>
          {list === null &&
            !error &&
            [0, 1, 2].map((i) => (
              <li key={`sk${i}`} className="picker-skeleton" aria-hidden>
                <span className="sk title" />
                <span className="sk" />
                <span className="sk short" />
              </li>
            ))}
          {sorted.map((s) => {
            const state = sessionState(s);
            return (
              <li key={s.slug}>
                <a className="picker-item" href={sessionHref(s.slug)}>
                  <span className="picker-head">
                    <b className="picker-title">{s.title || "엑셀 취합"}</b>
                    {s.mine && <span className="pill mine">내 담당</span>}
                  </span>
                  <span className="picker-meta">
                    {s.requester && <span>{s.requester} 님의 요청</span>}
                    {s.deadline && <span>마감 {formatDeadline(s.deadline)}</span>}
                    <span>● 접속 중 {s.online}명</span>
                    {state && <span className="warn">{state}</span>}
                  </span>
                  {s.description && <span className="picker-desc">{s.description}</span>}
                </a>
              </li>
            );
          })}
        </ul>
      </div>
    </div>
  );
}
