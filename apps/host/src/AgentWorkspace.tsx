import type { AgentServiceView } from "@mm/shared";
import type { ReactNode } from "react";

import { AgentPanel, agentStateLabel } from "./AgentPanel";

export { STATE_LABEL, agentStateLabel } from "./AgentPanel";

interface Props {
  view: AgentServiceView;
  /** 사이드바 위쪽(업무 목록·에이전트에서 진행 중). */
  sidebar: ReactNode;
  banner?: ReactNode;
  toast: (msg: string) => void;
  version: string;
  /** 목록에서 지웠다. */
  onForgotten: () => void;
}

/**
 * 업무 화면으로 다룰 수 없는 에이전트 서비스(넘기는 중·연결할 수 없음·끝남·에이전트가 거절함): 진행·저장만 보인다.
 * 서비스 중이고 관리자 연결이 있으면 이 PC 업무와 같은 업무 화면([`JobWorkspace`](./JobWorkspace))으로 다룬다.
 */
export function AgentWorkspace({ view, sidebar, banner, toast, version, onForgotten }: Props) {
  const s = view.status;
  const state = s?.state;
  return (
    <div className="shell">
      <header className="titlebar">
        <div className="brand">
          <span className="logo" />
          <b>Merge Master</b>
        </div>
        <span className="divider" />
        <span className="file">
          {view.workbook} · 에이전트 {view.agentName}
        </span>
        <span className="spacer" />
        <div className={`service-badge${state === "running" ? " on" : ""}`} aria-live="polite">
          에이전트 · {agentStateLabel(view)}
          {state === "running" && s ? ` · 접속 ${s.online} / ${s.roster}` : ""}
        </div>
      </header>
      <div className="body">
        <nav className="steps" aria-label="업무">
          {sidebar}
          <span className="spacer" />
          {version && <div className="app-version">v{version}</div>}
        </nav>
        <div className="main">
          <div className="step-head">
            <div>
              <div className="step-no">에이전트에서 진행 중</div>
              <h1>{view.title || "새 취합"}</h1>
              <p>
                에이전트 PC({view.agentName} · {view.addr.replace(/^http:\/\//, "")})가 마감까지 서비스하고, 마감 시각이
                되면 엑셀로 저장하고 끝냅니다. 이 PC를 꺼도 계속됩니다.
                {state === "running" &&
                  " 에이전트에 연결되면 설정·모니터링·채팅·히스토리를 이 PC 업무처럼 다룰 수 있어요."}
              </p>
            </div>
          </div>
          <main className="content">
            {banner}
            <AgentPanel view={view} toast={toast} onForgotten={onForgotten} />
          </main>
        </div>
      </div>
    </div>
  );
}
