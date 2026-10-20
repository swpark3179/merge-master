import type { AgentServiceView, JobSummary } from "@mm/shared";
import { formatDeadline } from "@mm/shared";
import { getVersion } from "@tauri-apps/api/app";
import { listen } from "@tauri-apps/api/event";
import { useCallback, useEffect, useRef, useState } from "react";

import { AgentWorkspace, agentStateLabel } from "./AgentWorkspace";
import { appApi, errorText, jobApi } from "./api";
import { JobProvider } from "./job";
import type { Step } from "./JobWorkspace";
import { JobWorkspace } from "./JobWorkspace";

/**
 * 호스트 앱: 여러 취합 업무를 함께 진행한다. 사이드바 위쪽의 업무 목록에서 업무를 고르면 그 업무의 단계 화면
 * ([`JobWorkspace`], 업무마다 새로 그림)이 보인다. 앱을 켜면 끝내지 않은 업무를 모두 되살린다.
 */
export function App() {
  const [jobs, setJobs] = useState<JobSummary[]>([]);
  const [job, setJob] = useState<string | null>(null);
  /** 앱을 켜며 되살리지 못한 업무·다시 공개하지 못한 까닭. */
  const [banner, setBanner] = useState<string | null>(null);
  /** 업무마다 마지막으로 본 단계·채팅 번호. */
  const steps = useRef(new Map<string, Step>());
  const [chatSeen, setChatSeen] = useState<Record<string, number>>({});
  const [excelRunning, setExcelRunning] = useState<boolean | null>(null);
  const [version, setVersion] = useState("");
  const [toastText, setToastText] = useState("");
  /** 알림마다 새로 그려 나타나는 동작을 다시 한다(같은 글이 또 와도). */
  const [toastId, setToastId] = useState(0);
  const toastTimer = useRef(0);
  const [ending, setEnding] = useState<JobSummary | null>(null);
  /** 에이전트 PC로 넘긴 서비스들과 지금 보는 것(고르면 업무 대신 보인다). */
  const [agents, setAgents] = useState<AgentServiceView[]>([]);
  const [agentView, setAgentView] = useState<string | null>(null);

  const toast = useCallback((msg: string) => {
    setToastText(msg);
    setToastId((n) => n + 1);
    window.clearTimeout(toastTimer.current);
    toastTimer.current = window.setTimeout(() => setToastText(""), 2200);
  }, []);

  useEffect(() => {
    void getVersion()
      .then(setVersion)
      .catch(() => {});
    const off = listen<JobSummary[]>("jobs", (e) => setJobs(e.payload));
    void appApi
      .agentServices()
      .then(setAgents)
      .catch(() => {});
    const offAgents = listen<AgentServiceView[]>("agent-services", (e) => setAgents(e.payload));
    return () => {
      void off.then((f) => f());
      void offAgents.then((f) => f());
    };
  }, []);

  // 앱을 켤 때 한 번: 끝내지 않은 업무를 되살린다(백엔드가 한 번만 하므로 다시 불러도 같은 결과). 업무가 없으면 빈 업무.
  const restored = useRef(false);
  useEffect(() => {
    void appApi
      .jobsRestore()
      .then(async (r) => {
        const list = await appApi.jobsList();
        setJobs(list);
        const ok = r.jobs.filter((j) => !j.error);
        setJob((cur) => cur ?? ok[0]?.id ?? list[list.length - 1]?.id ?? null);
        if (restored.current) return;
        restored.current = true;
        if (ok.length > 0) toast(`지난 업무 ${ok.length}개를 이어서 열었습니다`);
        const problems = [
          ...r.jobs.filter((j) => j.error).map((j) => `되살리지 못한 업무(${j.id}): ${j.error}`),
          r.serviceError ? `취합은 이어서 했지만 공개를 다시 열지 못했습니다: ${r.serviceError}` : "",
        ].filter(Boolean);
        if (problems.length > 0) setBanner(problems.join(" · "));
      })
      .catch((e) => setBanner(errorText(e)));
  }, [toast]);

  const createJob = useCallback(async () => {
    try {
      const created = await appApi.jobCreate();
      setJobs(await appApi.jobsList());
      steps.current.set(created.id, "import");
      setAgentView(null);
      setJob(created.id);
    } catch (e) {
      setBanner(errorText(e));
    }
  }, []);

  // 보던 업무가 끝났으면(다른 창이 아니라 여기서만 끝내므로 대개 목록이 먼저 바뀜) 남은 업무로. 에이전트로 넘겨 업무가
  // 하나도 남지 않았으면 에이전트 서비스를 보다가 다른 것을 고를 때 새 업무를 만든다.
  useEffect(() => {
    if (!job || jobs.some((j) => j.id === job)) return;
    if (jobs.length > 0) setJob(jobs[jobs.length - 1]!.id);
    else if (!agentView) void createJob();
  }, [job, jobs, agentView, createJob]);

  const onHandedOff = useCallback(
    (view: AgentServiceView) => {
      setAgents((list) => [view, ...list.filter((a) => a.id !== view.id)]);
      setAgentView(view.id);
      toast("에이전트로 넘겼어요. 이 PC를 꺼도 서비스가 계속됩니다");
    },
    [toast],
  );

  const endJob = useCallback(
    async (id: string) => {
      try {
        const left = await appApi.jobEnd(id);
        setJobs(left);
        steps.current.delete(id);
        toast("업무를 끝냈습니다");
        if (left.length === 0) await createJob();
        else if (id === job) setJob(left[left.length - 1]!.id);
      } catch (e) {
        setBanner(errorText(e));
      }
    },
    [createJob, job, toast],
  );

  const onStep = useCallback((s: Step) => job && steps.current.set(job, s), [job]);
  const onChatSeen = useCallback(
    (id: number) => job && setChatSeen((m) => ({ ...m, [job]: Math.max(m[job] ?? 0, id) })),
    [job],
  );

  const sidebar = (
    <JobList
      jobs={jobs}
      current={agentView ? null : job}
      chatSeen={chatSeen}
      onPick={(id) => {
        setAgentView(null);
        setJob(id);
      }}
      onCreate={() => void createJob()}
      onEnd={setEnding}
      agents={agents}
      currentAgent={agentView}
      onPickAgent={setAgentView}
    />
  );
  const bannerNode = banner ? (
    <div className="recovery-card" role="region" aria-label="지난 업무">
      <span className="error small">{banner}</span>
      <span className="spacer" />
      <button className="ghost small" onClick={() => setBanner(null)}>
        닫기
      </button>
    </div>
  ) : null;

  const agent = agentView ? agents.find((a) => a.id === agentView) : undefined;
  return (
    <>
      {agent ? (
        <AgentWorkspace
          key={agent.id}
          view={agent}
          sidebar={sidebar}
          banner={bannerNode}
          toast={toast}
          version={version}
          onForgotten={() => {
            setAgentView(null);
            toast("목록에서 지웠어요");
          }}
        />
      ) : job ? (
        <JobProvider key={job} job={job}>
          <JobWorkspace
            jobs={jobs}
            initialStep={steps.current.get(job)}
            onStep={onStep}
            chatSeen={chatSeen[job] ?? 0}
            onChatSeen={onChatSeen}
            excelRunning={excelRunning}
            onExcelStatus={setExcelRunning}
            onNewJob={() => void createJob()}
            onEndJob={() => {
              const j = jobs.find((x) => x.id === job);
              if (j) setEnding(j);
            }}
            onHandedOff={onHandedOff}
            sidebar={sidebar}
            banner={bannerNode}
            toast={toast}
            version={version}
          />
        </JobProvider>
      ) : (
        <div className="shell starting">
          <div className="muted">지난 업무를 확인하는 중…</div>
          {bannerNode}
        </div>
      )}
      {toastText && (
        <div key={toastId} className="toast-host" role="status">
          {toastText}
        </div>
      )}
      {ending && (
        <EndJobDialog
          job={ending}
          onCancel={() => setEnding(null)}
          onConfirm={() => {
            const id = ending.id;
            setEnding(null);
            void endJob(id);
          }}
        />
      )}
    </>
  );
}

function percent(j: JobSummary): number | null {
  return j.required > 0 ? Math.round((j.filled / j.required) * 100) : null;
}

/** 사이드바 위쪽: 진행 중인 취합 업무 목록(업무명·파일·공유 상태·승인 대기·진행률), 에이전트에서 진행 중인 것. */
function JobList(props: {
  jobs: JobSummary[];
  current: string | null;
  chatSeen: Record<string, number>;
  onPick: (id: string) => void;
  onCreate: () => void;
  onEnd: (j: JobSummary) => void;
  agents: AgentServiceView[];
  currentAgent: string | null;
  onPickAgent: (id: string) => void;
}) {
  const { jobs, current, chatSeen, agents } = props;
  const activeAgents = agents.filter((a) => a.status?.state !== "done" && a.status?.state !== "cancelled").length;
  return (
    <div className="jobs" role="group" aria-label="취합 업무">
      <div className="steps-title">취합 업무 {jobs.length > 1 ? jobs.length : ""}</div>
      <ul className="job-list">
        {jobs.map((j) => {
          const pct = percent(j);
          const unread = j.id !== current && j.lastChat > (chatSeen[j.id] ?? 0);
          const name = j.title || "새 취합";
          return (
            <li key={j.id} className={`job-item${j.id === current ? " active" : ""}`} data-job={j.id}>
              <button
                className="job-pick"
                aria-current={j.id === current ? "page" : undefined}
                aria-label={`업무 ${name}`}
                title={j.sheets.length > 0 ? `${j.workbook} · ${j.sheets.join(", ")}` : "아직 불러오지 않음"}
                onClick={() => props.onPick(j.id)}
              >
                <span className="job-title">
                  {name}
                  {unread && <span className="job-dot" title="안 읽은 채팅" />}
                </span>
                <span className="job-file">{j.workbook || "파일 없음"}</span>
                <span className="job-chips">
                  {j.published ? (
                    <span className="job-chip on">
                      공유 중 {j.online}/{j.roster}
                    </span>
                  ) : (
                    j.loaded && <span className="job-chip">공유 전</span>
                  )}
                  {j.pending > 0 && <span className="job-chip warn">승인 대기 {j.pending}</span>}
                  {pct !== null && <span className="job-chip">{pct}%</span>}
                  {j.closed && <span className="job-chip">마감됨</span>}
                </span>
              </button>
              <button
                className="job-end"
                aria-label={`${name} 업무 끝내기`}
                title="업무 끝내기"
                onClick={() => props.onEnd(j)}
              >
                ×
              </button>
            </li>
          );
        })}
      </ul>
      <button className="ghost small new-job" onClick={props.onCreate}>
        + 새 취합
      </button>
      {agents.length > 0 && (
        <>
          <div className="steps-title">에이전트에서 진행 중 {activeAgents > 0 ? activeAgents : ""}</div>
          <ul className="job-list agent-list" aria-label="에이전트에서 진행 중">
            {agents.map((a) => {
              const s = a.status;
              const name = a.title || "새 취합";
              const pct = s && s.required > 0 ? Math.round((s.filled / s.required) * 100) : null;
              const label = agentStateLabel(a);
              const warn = !a.reachable || s?.state === "failed";
              return (
                <li key={a.id} className={`job-item${a.id === props.currentAgent ? " active" : ""}`} data-agent={a.id}>
                  <button
                    className="job-pick"
                    aria-current={a.id === props.currentAgent ? "page" : undefined}
                    aria-label={`에이전트 업무 ${name}`}
                    title={`${a.agentName} · ${a.workbook}`}
                    onClick={() => props.onPickAgent(a.id)}
                  >
                    <span className="job-title">{name}</span>
                    <span className="job-file">
                      {a.agentName} · {a.workbook}
                    </span>
                    <span className="job-chips">
                      <span className={`job-chip${s?.state === "running" ? " on" : warn ? " warn" : ""}`}>
                        {s?.state === "running" ? `공유 중 ${s.online}/${s.roster}` : label}
                      </span>
                      {s && s.pending.length > 0 && <span className="job-chip warn">승인 대기 {s.pending.length}</span>}
                      {pct !== null && <span className="job-chip">{pct}%</span>}
                      {s && s.state === "running" && (
                        <span className="job-chip">마감 {formatDeadline(s.deadline)}</span>
                      )}
                      {s?.state === "running" && !a.reachable && <span className="job-chip warn">연결 안 됨</span>}
                    </span>
                  </button>
                </li>
              );
            })}
          </ul>
        </>
      )}
    </div>
  );
}

/** '업무 끝내기' 확인: 저장하지 않은 변경·공개 중인지 알려 준다. */
function EndJobDialog({ job, onCancel, onConfirm }: { job: JobSummary; onCancel: () => void; onConfirm: () => void }) {
  const [unsaved, setUnsaved] = useState<number | null>(null);
  useEffect(() => {
    if (!job.loaded) return;
    void jobApi(job.id)
      .saveInfo()
      .then((i) =>
        setUnsaved(i.lastSave ? i.summary.historyEntries - i.lastSave.historyEntries : i.summary.changedCells),
      )
      .catch(() => {});
  }, [job]);
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && onCancel();
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onCancel]);
  const name = job.title || "새 취합";
  return (
    <div className="modal-back" onClick={onCancel}>
      <div
        className="modal card end-job"
        role="alertdialog"
        aria-label="업무 끝내기"
        onClick={(e) => e.stopPropagation()}
      >
        <b>‘{name}’ 업무를 끝낼까요?</b>
        <ul className="small">
          {job.published && <li>공유를 멈추고, 접속 중인 참여자 화면에는 ‘업무가 끝났습니다’가 보입니다.</li>}
          {unsaved !== null && unsaved > 0 && (
            <li className="warn">
              저장하지 않은 변경이 {unsaved.toLocaleString()}건 있습니다. 먼저 6 저장에서 엑셀로 저장하세요.
            </li>
          )}
          <li>앱을 다시 켜도 이어서 열지 않습니다(기록 파일은 남습니다).</li>
        </ul>
        <div className="row">
          <span className="spacer" />
          <button className="ghost" onClick={onCancel}>
            취소
          </button>
          <button className="danger" onClick={onConfirm} autoFocus>
            업무 끝내기
          </button>
        </div>
      </div>
    </div>
  );
}
