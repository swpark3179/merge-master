import type { AgentServiceView, ServiceState } from "@mm/shared";
import { formatDeadline, remainingAt, remainingTick } from "@mm/shared";
import type { ReactNode } from "react";
import { useEffect, useState } from "react";

import { appApi, errorText } from "./api";

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

export const STATE_LABEL: Record<ServiceState, string> = {
  running: "서비스 중",
  saving: "저장 중",
  done: "저장하고 끝냄",
  failed: "저장 실패",
  cancelled: "저장 없이 끝냄",
};

/** 사이드바 한 줄의 상태 글자. */
export function agentStateLabel(v: AgentServiceView): string {
  if (v.pending) return "넘기는 중";
  if (!v.status) return v.reachable ? "확인하는 중" : "연결 안 됨";
  return STATE_LABEL[v.status.state];
}

function time(ms: number | undefined): string {
  if (!ms) return "";
  const d = new Date(ms);
  return `${d.getMonth() + 1}/${d.getDate()} ${String(d.getHours()).padStart(2, "0")}:${String(d.getMinutes()).padStart(2, "0")}`;
}

type Ask = { title: string; body: string; ok: string; danger?: boolean; run: () => Promise<unknown> };

/**
 * 에이전트 PC에서 진행 중인 서비스 하나: 주소·마감·진행률·승인 대기·저장할 곳, 지금 저장하고 끝내기 / 저장 없이 끝내기.
 * 모니터링 상세·채팅·공지·히스토리·설정은 에이전트로 넘긴 업무에서는 볼 수 없다.
 */
export function AgentWorkspace({ view, sidebar, banner, toast, version, onForgotten }: Props) {
  const s = view.status;
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [ask, setAsk] = useState<Ask | null>(null);
  // 남은 시간은 에이전트 시계로(받은 때의 차이를 그대로 쓴다).
  const [skew, setSkew] = useState(0);
  useEffect(() => {
    if (s) setSkew(s.nowMs - Date.now());
  }, [s]);
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const id = window.setTimeout(() => setNow(Date.now()), remainingTick(s?.deadlineAtMs, now + skew));
    return () => window.clearTimeout(id);
  }, [now, s?.deadlineAtMs, skew]);
  useEffect(() => {
    if (!ask) return;
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && !busy && setAsk(null);
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [ask, busy]);

  const run = async (f: () => Promise<unknown>, done?: string) => {
    setBusy(true);
    setError(null);
    try {
      await f();
      if (done) toast(done);
    } catch (e) {
      setError(errorText(e));
    } finally {
      setBusy(false);
      setAsk(null);
    }
  };

  const copy = async (text: string, msg: string) => {
    try {
      await navigator.clipboard.writeText(text);
      toast(msg);
    } catch (e) {
      setError(errorText(e));
    }
  };

  const url = s?.urls[0];
  const listUrl = s?.listUrls[0];
  const notice = () =>
    [
      `[${view.title || "엑셀 취합"}] 취합 요청`,
      s?.deadline ? `마감: ${formatDeadline(s.deadline)}` : "",
      `접속: ${url}`,
      "같은 사내망에서 위 주소를 여세요. 등록된 PC는 바로 들어오고, 그 밖은 이름을 입력하면 호스트가 승인합니다.",
    ]
      .filter(Boolean)
      .join("\n");
  const state = s?.state;
  const finished = state === "done" || state === "cancelled";
  const pct = s && s.required > 0 ? Math.round((s.filled / s.required) * 100) : null;
  const left = s ? remainingAt(s.deadlineAtMs, now + skew) : "";

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
              </p>
            </div>
          </div>
          <main className="content">
            {banner}
            <div className="screen agent-screen">
              {(view.problem || view.pending) && (
                <div className={view.reachable ? "warn small" : "error small"} role="status">
                  {view.pending ? "에이전트가 받았는지 확인하는 중이에요." : view.problem}
                  {!view.reachable && s && " (아래는 마지막으로 받은 상태예요)"}
                </div>
              )}
              <div className="service-top">
                <section className="card" aria-label="진행">
                  <div className="card-head">
                    <span className="card-title">진행</span>
                    <span className="spacer" />
                    <span className={`pill${state === "running" ? " ok-pill" : ""}`}>{agentStateLabel(view)}</span>
                  </div>
                  {s ? (
                    <div className="kv-list">
                      <div>
                        <span className="muted">마감</span>
                        <b>{formatDeadline(s.deadline)}</b>
                        {state === "running" && (
                          <span className={left === "마감 지남" ? "warn" : "muted"}> · {left}</span>
                        )}
                        <span className="muted small"> (에이전트 PC 시각)</span>
                      </div>
                      <div>
                        <span className="muted">접속</span>
                        {state === "running" ? `${s.online} / ${s.roster}명` : `명단 ${s.roster}명`}
                      </div>
                      <div>
                        <span className="muted">필수 칸</span>
                        {s.required > 0
                          ? `${s.filled.toLocaleString()} / ${s.required.toLocaleString()} (${pct}%)`
                          : "없음"}
                      </div>
                      {state === "running" && s.recalc.state !== "none" && (
                        <div>
                          <span className="muted">수식 계산</span>
                          {s.recalc.state === "ready"
                            ? `에이전트의 Excel이 바로 계산 (수식 ${s.recalc.formulas.toLocaleString()}칸)`
                            : s.recalc.state === "opening"
                              ? "계산 사본을 여는 중…"
                              : "저장할 때 계산"}
                        </div>
                      )}
                      <div>
                        <span className="muted">저장할 곳</span>
                        <span className="mono small">{s.savedPath ?? s.outputPath}</span>
                      </div>
                      {s.savedAtMs && (
                        <div>
                          <span className="muted">저장함</span>
                          {time(s.savedAtMs)}
                        </div>
                      )}
                      {s.lastError && (
                        <div className="warn small">
                          저장 {s.attempts}번 시도 · 마지막 오류: {s.lastError}
                          {state === "saving" && " (1분 뒤 다시 저장합니다)"}
                        </div>
                      )}
                      {s.lastChatMs && (
                        <div className="muted small">
                          참여자 채팅이 있어요(마지막 {time(s.lastChatMs)}). 에이전트로 넘긴 업무의 채팅은 이 앱에서 볼
                          수 없어요.
                        </div>
                      )}
                    </div>
                  ) : (
                    <div className="muted">{view.reachable ? "상태를 확인하는 중…" : "아직 상태를 받지 못했어요."}</div>
                  )}
                  <div className="row agent-actions">
                    {(state === "running" || state === "failed") && (
                      <button
                        className="ghost"
                        disabled={busy || !view.reachable}
                        onClick={() =>
                          setAsk({
                            title: "저장하지 않고 끝낼까요?",
                            body: "취합한 값을 엑셀로 저장하지 않고 서비스를 끝냅니다. 참여자 화면에는 ‘업무가 끝났습니다’가 보여요.",
                            ok: "저장 없이 끝내기",
                            danger: true,
                            run: () => appApi.agentFinish(view.id, false),
                          })
                        }
                      >
                        저장 없이 끝내기
                      </button>
                    )}
                    <span className="spacer" />
                    {state === "failed" && (
                      <button
                        disabled={busy || !view.reachable}
                        onClick={() => void run(() => appApi.agentRetry(view.id), "다시 저장하고 있어요")}
                      >
                        다시 저장
                      </button>
                    )}
                    {state === "running" && (
                      <button
                        disabled={busy || !view.reachable}
                        onClick={() =>
                          setAsk({
                            title: "지금 저장하고 끝낼까요?",
                            body: "마감 전이라도 지금 엑셀로 저장하고 서비스를 끝냅니다. 참여자는 더 입력할 수 없어요.",
                            ok: "지금 저장하고 끝내기",
                            run: () => appApi.agentFinish(view.id, true),
                          })
                        }
                      >
                        지금 저장하고 끝내기
                      </button>
                    )}
                    {(finished || (!view.reachable && !view.pending) || view.problem) && (
                      <button
                        className="ghost"
                        disabled={busy}
                        onClick={() =>
                          finished
                            ? void run(async () => {
                                await appApi.agentForget(view.id);
                                onForgotten();
                              })
                            : setAsk({
                                title: "목록에서 지울까요?",
                                body: "이 앱의 목록에서만 지웁니다. 에이전트의 서비스는 그대로이고, 다시는 이 앱에서 볼 수 없어요.",
                                ok: "목록에서 지우기",
                                danger: true,
                                run: async () => {
                                  await appApi.agentForget(view.id);
                                  onForgotten();
                                },
                              })
                        }
                      >
                        목록에서 지우기
                      </button>
                    )}
                  </div>
                  {error && <div className="error small">{error}</div>}
                </section>

                <section className="card share" aria-label="공유">
                  {state === "running" && url ? (
                    <>
                      <div className="card-head">
                        <span className="card-title">참여자에게 보낼 주소</span>
                        <span className="spacer" />
                        <span className="pill ok-pill">공유 중</span>
                      </div>
                      <div className="url-row">
                        <code className="url">{url}</code>
                        <button onClick={() => void copy(url, "주소를 복사했어요")}>복사</button>
                      </div>
                      {listUrl && (
                        <div className="url-row list-url">
                          <span className="muted small">업무 목록</span>
                          <code className="url">{listUrl}</code>
                          <button className="ghost" onClick={() => void copy(listUrl, "업무 목록 주소를 복사했어요")}>
                            복사
                          </button>
                        </div>
                      )}
                      <button className="ghost" onClick={() => void copy(notice(), "안내 문구를 복사했어요")}>
                        안내 문구 복사
                      </button>
                    </>
                  ) : (
                    <div className="muted">{finished ? "서비스를 끝냈어요." : "지금은 공유하지 않아요."}</div>
                  )}
                </section>
              </div>

              {s && state === "running" && (
                <section className="card agent-pending" aria-label="승인 대기">
                  <div className="card-title">승인 대기 {s.pending.length > 0 ? s.pending.length : ""}</div>
                  {s.pending.length === 0 ? (
                    <div className="muted small">
                      IP 명단의 PC는 그 이름으로 바로 들어오고, 그 밖은 이름을 입력하면 여기서 승인해요(이 PC가 꺼져
                      있으면 승인할 때까지 기다립니다).
                    </div>
                  ) : (
                    <ul className="pending-list">
                      {s.pending.map((p) => (
                        <li key={p.name}>
                          <b>{p.name}</b> <span className="muted small">{p.ip}</span>
                          <span className="spacer" />
                          <button
                            className="ghost small"
                            disabled={busy}
                            onClick={() =>
                              void run(() => appApi.agentReject(view.id, p.name), `${p.name} 님을 거부했어요`)
                            }
                          >
                            거부
                          </button>
                          <button
                            className="small"
                            disabled={busy}
                            onClick={() =>
                              void run(() => appApi.agentApprove(view.id, p.name), `${p.name} 님을 들였어요`)
                            }
                          >
                            승인
                          </button>
                        </li>
                      ))}
                    </ul>
                  )}
                </section>
              )}
            </div>
          </main>
        </div>
      </div>
      {ask && (
        <div className="modal-back" onClick={() => !busy && setAsk(null)}>
          <div
            className="modal card end-job"
            role="alertdialog"
            aria-label={ask.title}
            onClick={(e) => e.stopPropagation()}
          >
            <b>{ask.title}</b>
            <p className="small">{ask.body}</p>
            <div className="row">
              <span className="spacer" />
              <button className="ghost" onClick={() => setAsk(null)} disabled={busy}>
                취소
              </button>
              <button
                className={ask.danger ? "danger" : undefined}
                disabled={busy}
                onClick={() => void run(ask.run)}
                autoFocus
              >
                {ask.ok}
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
