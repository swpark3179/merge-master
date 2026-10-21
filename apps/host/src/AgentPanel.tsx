import type { AgentServiceView, ServiceState } from "@mm/shared";
import { formatDeadline, remainingAt, remainingTick } from "@mm/shared";
import { save as saveDialog } from "@tauri-apps/plugin-dialog";
import { useEffect, useState } from "react";

import { appApi, errorText } from "./api";

export const STATE_LABEL: Record<ServiceState, string> = {
  running: "서비스 중",
  saving: "저장 중",
  done: "저장하고 끝냄",
  failed: "저장 실패",
  awaitingHost: "관리자 저장 대기",
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

/** 에이전트 시계로 남은 시간을 잰다(받은 때의 차이를 그대로 쓴다). */
export function useAgentNow(nowMs: number | undefined, deadlineAtMs: number | undefined): number {
  const [skew, setSkew] = useState(0);
  useEffect(() => {
    if (nowMs !== undefined) setSkew(nowMs - Date.now());
  }, [nowMs]);
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const id = window.setTimeout(() => setNow(Date.now()), remainingTick(deadlineAtMs, now + skew));
    return () => window.clearTimeout(id);
  }, [now, deadlineAtMs, skew]);
  return now + skew;
}

type Ask = { title: string; body: string; ok: string; danger?: boolean; run: () => Promise<unknown> };

interface Props {
  view: AgentServiceView;
  toast: (msg: string) => void;
  /** 목록에서 지웠다. */
  onForgotten: () => void;
}

/**
 * 에이전트 PC에서 진행 중인 서비스 하나의 진행·저장: 주소·마감·진행률·저장할 곳, 지금 저장하고 끝내기 / 저장 없이 끝내기 /
 * 다시 저장 / 목록에서 지우기, 이 PC에 저장(서비스 중이면 지금 상태, 관리자 저장 대기면 에이전트가 남긴 취합 데이터).
 * 업무 화면(관리자 연결)의 마지막 단계이자, 연결할 수 없을 때의 화면이다.
 */
export function AgentPanel({ view, toast, onForgotten }: Props) {
  const s = view.status;
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [ask, setAsk] = useState<Ask | null>(null);
  /** 이 화면에서 마지막으로 이 PC에 저장한 파일. */
  const [savedHere, setSavedHere] = useState<string | null>(null);
  const agentNow = useAgentNow(s?.nowMs, s?.deadlineAtMs);
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

  /** 이 PC에 저장한다: `result`면 관리자 저장 대기의 취합 데이터(에이전트에 알려 그쪽 데이터를 지운다), 아니면 지금 상태. */
  const saveHere = async (result: boolean) => {
    setError(null);
    const defaultPath = await appApi.agentSaveDefault(view.id).catch(() => undefined);
    const ext = (view.workbook.split(".").pop() ?? "").toLowerCase() || "xlsx";
    let picked: string | null;
    try {
      picked = await saveDialog({ defaultPath, filters: [{ name: `Excel 통합문서 (.${ext})`, extensions: [ext] }] });
    } catch (e) {
      setError(errorText(e));
      return;
    }
    if (!picked) return;
    await run(async () => {
      const path = picked;
      const r = result ? await appApi.agentSaveResult(view.id, path) : await appApi.agentSaveNow(view.id, path);
      setSavedHere(r.path);
      toast(
        !result
          ? "지금 상태를 이 PC에 저장했어요"
          : r.told
            ? "이 PC에 저장하고 에이전트의 취합 데이터를 지웠어요"
            : "이 PC에 저장했어요. 에이전트에 연결되면 알려 그쪽 데이터를 지울게요",
      );
    });
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
      "같은 사내망에서 위 주소를 여세요. 등록된 PC는 바로 들어오고, 그 밖은 이름을 입력하면 그 이름으로 바로 참여합니다.",
    ]
      .filter(Boolean)
      .join("\n");
  const state = s?.state;
  const finished = state === "done" || state === "cancelled";
  const waiting = state === "awaitingHost";
  const pct = s && s.required > 0 ? Math.round((s.filled / s.required) * 100) : null;
  const left = s ? remainingAt(s.deadlineAtMs, agentNow) : "";

  return (
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
                {state === "running" && <span className={left === "마감 지남" ? "warn" : "muted"}> · {left}</span>}
                <span className="muted small"> (에이전트 PC 시각)</span>
              </div>
              <div>
                <span className="muted">접속</span>
                {state === "running" ? `${s.online} / ${s.roster}명` : `명단 ${s.roster}명`}
                {state === "running" && s.pending.length > 0 && (
                  <span className="warn small"> · 승인 대기 {s.pending.length}명</span>
                )}
              </div>
              <div>
                <span className="muted">필수 칸</span>
                {s.required > 0 ? `${s.filled.toLocaleString()} / ${s.required.toLocaleString()} (${pct}%)` : "없음"}
              </div>
              {state === "running" && s.recalc.state !== "none" && (
                <div>
                  <span className="muted">수식 계산</span>
                  저장할 때 계산 <span className="muted small">(에이전트 PC는 Excel을 열지 않아요)</span>
                </div>
              )}
              <div>
                <span className="muted">저장할 곳</span>
                <span className="mono small">{s.savedPath ?? s.outputPath}</span>
                <span className="muted small"> ({s.savedOnHost ? "이 PC" : "에이전트 PC"})</span>
              </div>
              {s.savedAtMs && (
                <div>
                  <span className="muted">저장함</span>
                  {time(s.savedAtMs)}
                  {s.savedOnHost && s.savedPath && (
                    <button className="ghost small" onClick={() => void appApi.revealFile(s.savedPath ?? "")}>
                      파일 보기
                    </button>
                  )}
                </div>
              )}
              {!s.excel && state === "running" && (
                <div className="muted small">
                  에이전트 PC에 Excel이 없어요. 마감 때 취합 데이터를 남겨 두면 이 앱이 접속할 때 이 PC에서 저장해요.
                </div>
              )}
              {s.lastError && (
                <div className="warn small">
                  저장 {s.attempts}번 시도 · 마지막 오류: {s.lastError}
                  {state === "saving" && " (1분 뒤 다시 저장합니다)"}
                </div>
              )}
              {waiting && (
                <div className="warn small" role="status">
                  {s.excel ? "에이전트 PC에서 엑셀로 저장하지 못했어요." : "에이전트 PC에 Excel이 없어요."} 취합한
                  데이터는 에이전트 PC에 남아 있어요{view.resultHere ? "(이 PC에도 받아 두었어요)" : ""}. ‘이 PC에
                  저장’으로 저장하면 에이전트의 데이터를 지우고 끝냅니다.
                </div>
              )}
              {view.savedHere && (
                <div className="muted small">
                  이 PC에 저장함: <span className="mono">{view.savedHere}</span> — 에이전트에 연결되면 알려 그쪽
                  데이터를 지워요.
                </div>
              )}
              {savedHere && state === "running" && (
                <div className="small">
                  이 PC에 저장함: <span className="mono">{savedHere}</span>{" "}
                  <button className="ghost small" onClick={() => void appApi.revealFile(savedHere)}>
                    파일 보기
                  </button>
                </div>
              )}
              {s.lastChatMs && !view.live && (
                <div className="muted small">
                  참여자 채팅이 있어요(마지막 {time(s.lastChatMs)}). 에이전트에 연결되면 모니터링에서 볼 수 있어요.
                </div>
              )}
            </div>
          ) : (
            <div className="muted">{view.reachable ? "상태를 확인하는 중…" : "아직 상태를 받지 못했어요."}</div>
          )}
          <div className="row agent-actions">
            {(state === "running" || state === "failed" || waiting) && (
              <button
                className="ghost"
                disabled={busy || !view.reachable || !!view.savedHere}
                onClick={() =>
                  setAsk({
                    title: "저장하지 않고 끝낼까요?",
                    body: waiting
                      ? "에이전트 PC에 남은 취합 데이터를 지우고 서비스를 끝냅니다. 되돌릴 수 없어요."
                      : "취합한 값을 엑셀로 저장하지 않고 서비스를 끝냅니다. 참여자 화면에는 ‘업무가 끝났습니다’가 보여요.",
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
            {(state === "failed" || (waiting && s?.excel)) && (
              <button
                className={waiting ? "ghost" : undefined}
                disabled={busy || !view.reachable || !!view.savedHere}
                onClick={() => void run(() => appApi.agentRetry(view.id), "에이전트 PC에서 다시 저장하고 있어요")}
              >
                {waiting ? "에이전트 PC에서 다시 저장" : "다시 저장"}
              </button>
            )}
            {waiting && (
              <button
                disabled={busy || !!view.savedHere || (!view.reachable && !view.resultHere)}
                onClick={() => void saveHere(true)}
              >
                이 PC에 저장…
              </button>
            )}
            {state === "running" && (
              <button className="ghost" disabled={busy || !view.reachable} onClick={() => void saveHere(false)}>
                이 PC에 지금 상태 저장…
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
            {(finished || (!view.reachable && !view.pending) || (view.problem && !view.live)) && (
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
