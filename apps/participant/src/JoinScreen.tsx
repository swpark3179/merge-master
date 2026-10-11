import { formatDeadline } from "@mm/shared";
import { useState } from "react";

import type { SessionState } from "./useSession";

const KICK_TEXT = {
  switchedDevice: "다른 기기(창)에서 같은 이름으로 접속해 이 화면의 연결이 끊겼습니다.",
  host: "호스트가 연결을 끊었습니다.",
  removed: "명단에서 빠졌습니다. 호스트에게 문의해 주세요.",
} as const;

/**
 * 1e 접속 화면: 호스트의 IP 명단에 있는 PC는 이름을 묻지 않고 그 이름으로 바로 들어간다. 그 밖에는 이름을 입력하고
 * 호스트 승인을 기다린다. 승인 대기·중복 접속·끊김 상태도 여기서 보인다.
 */
export function JoinScreen({ s }: { s: SessionState }) {
  const [name, setName] = useState(s.savedName ?? "");
  const { info, phase } = s;
  const busy = phase.kind === "joining";

  if (phase.kind === "ended") {
    return (
      <div className="join-wrap">
        <div className="join-card" role="status">
          <div className="muted">{info?.requester ? `${info.requester} 님의` : "호스트의"} 엑셀 취합 요청</div>
          <h1>{info?.title || "엑셀 취합"}</h1>
          <div className="desc">호스트가 이 업무를 끝냈습니다. 더 입력할 수 없습니다.</div>
          <a className="back-link" href="/">
            ← 다른 취합 업무 보기
          </a>
        </div>
      </div>
    );
  }

  // 공개 중지(곧 다시 열릴 수 있음)는 아래 '서비스가 중지되었습니다'로 따로 알린다.
  const missing = s.missing && phase.kind !== "closed";
  return (
    <div className="join-wrap">
      <form
        className="join-card"
        onSubmit={(e) => {
          e.preventDefault();
          s.join(name);
        }}
      >
        <div className="muted">{info?.requester ? `${info.requester} 님의` : "호스트의"} 엑셀 취합 요청</div>
        <h1>{info?.title || "엑셀 취합"}</h1>
        <div className="chips">
          {info?.deadline && <span className="pill">마감 {formatDeadline(info.deadline)}</span>}
          <span className="pill">● 접속 중 {info?.online ?? 0}명</span>
        </div>
        {info?.description && <div className="desc">{info.description}</div>}
        <label className="name-field">
          <b>이름</b>
          <input
            autoFocus
            aria-label="이름"
            value={name}
            maxLength={20}
            disabled={busy}
            onChange={(e) => setName(e.target.value)}
          />
        </label>
        <div className="hint">
          호스트가 승인하면 참여합니다. 호스트 명단과 같은 이름이면 담당 행이 자동으로 연결됩니다(호스트가 등록한 PC는
          이름 없이 바로 들어갑니다).
        </div>
        {phase.kind === "name" && phase.error && <div className="error">{phase.error}</div>}
        <button type="submit" className="primary" disabled={busy || !name.trim() || !s.connected}>
          {busy ? "참여하는 중…" : "참여하기"}
        </button>
        <div className="note">입력 내용과 변경 이력은 호스트 PC에 저장됩니다.</div>
        <a className="back-link" href="/">
          ← 다른 취합 업무 보기
        </a>
      </form>

      <div className="join-side">
        {missing && (
          <div className="state-card" role="status">
            <b>이 업무는 지금 공개되어 있지 않습니다</b>
            <div className="muted">주소가 맞는지 확인해 주세요. 호스트가 이 업무를 공개하면 자동으로 연결합니다.</div>
            <a className="back-link" href="/">
              공개 중인 업무 목록 보기
            </a>
          </div>
        )}
        {!missing && !s.connected && phase.kind !== "kicked" && phase.kind !== "closed" && (
          <div className="state-card">
            <b>연결 중</b>
            <div className="muted">호스트 PC에 연결하는 중입니다… 같은 사내망인지 확인해 주세요.</div>
          </div>
        )}
        {phase.kind === "joining" && phase.known && (
          <div className="state-card" role="status">
            <b>등록된 PC</b>
            <div>‘{phase.name}’ 님으로 들어가는 중입니다…</div>
          </div>
        )}
        {phase.kind === "pending" && (
          <div className="state-card" role="status">
            <b>호스트 승인 대기</b>
            <div>‘{phase.name}’ 님, 호스트 승인을 기다리는 중입니다…</div>
            <div className="bar indeterminate">
              <span />
            </div>
            <button type="button" className="link" onClick={s.reset}>
              이름 다시 입력
            </button>
          </div>
        )}
        {phase.kind === "dup" && (
          <div className="state-card" role="alertdialog" aria-label="같은 이름이 이미 접속 중">
            <b>같은 이름이 이미 접속 중</b>
            <div>다른 기기에서 ‘{phase.name}’(으)로 접속 중입니다. 이 기기로 전환할까요?</div>
            <div className="row">
              <button type="button" className="primary" onClick={s.switchDevice}>
                이 기기로 전환
              </button>
              <button type="button" onClick={s.reset}>
                취소
              </button>
            </div>
          </div>
        )}
        {phase.kind === "kicked" && (
          <div className="state-card" role="alert">
            <b>연결이 끊겼습니다</b>
            <div>{KICK_TEXT[phase.reason]}</div>
            {phase.reason !== "removed" && s.savedName && (
              <button type="button" className="primary" onClick={() => s.join(s.savedName!)}>
                {phase.reason === "switchedDevice" ? "이 기기로 다시 전환" : "다시 참여"}
              </button>
            )}
          </div>
        )}
        {phase.kind === "closed" && (
          <div className="state-card" role="status">
            <b>서비스가 중지되었습니다</b>
            <div className="muted">호스트가 다시 열면 자동으로 연결합니다.</div>
            <a className="back-link" href="/">
              다른 취합 업무 보기
            </a>
          </div>
        )}
      </div>
    </div>
  );
}
