import type {
  AgentServiceView,
  CommsSnapshot,
  HostEvent,
  HostSnapshot,
  JobSummary,
  ServiceInfo,
  SheetModel,
} from "@mm/shared";
import type { ReactNode } from "react";
import { useCallback, useEffect, useMemo, useState } from "react";

import { AgentPanel, agentStateLabel } from "./AgentPanel";
import { errorText } from "./api";
import type { FooterOverride } from "./frame";
import { FrameContext } from "./frame";
import { useApi, useJobListen } from "./job";
import { ConfigScreen } from "./screens/config/ConfigScreen";
import { HistoryScreen } from "./screens/history/HistoryScreen";
import { ImportScreen } from "./screens/import/ImportScreen";
import { MonitorScreen } from "./screens/monitor/MonitorScreen";
import { SaveScreen } from "./screens/save/SaveScreen";
import { ServiceScreen } from "./screens/service/ServiceScreen";

export type Step = "import" | "config" | "service" | "monitor" | "history" | "save" | "agent";

type StepInfo = { id: Step; label: string; desc: string };

const STEPS: StepInfo[] = [
  { id: "import", label: "불러오기", desc: "참여자와 함께 채울 엑셀 파일을 고르세요." },
  { id: "config", label: "설정", desc: "참여자가 어느 칸을 채울지 정하세요." },
  { id: "service", label: "서비스", desc: "주소를 공유하면 참여자가 브라우저로 접속합니다." },
  { id: "monitor", label: "모니터링", desc: "참여자가 입력하는 내용을 실시간으로 확인하세요." },
  { id: "history", label: "히스토리", desc: "누가 언제 무엇을 바꿨는지 확인하세요." },
  { id: "save", label: "저장", desc: "모은 내용을 엑셀 파일로 저장하세요." },
];

/** 에이전트 업무: 저장 대신 에이전트(마감 시각에 에이전트 PC가 저장하고 끝낸다). */
const AGENT_STEPS: StepInfo[] = STEPS.map((s) =>
  s.id === "save"
    ? {
        id: "agent",
        label: "에이전트",
        desc: "마감 시각이 되면 에이전트 PC가 엑셀로 저장하고 끝냅니다. 지금 저장하고 끝낼 수도 있어요.",
      }
    : s,
);

/** 아직 만들지 않은 단계. */
const NOT_YET: Step[] = [];

interface Props {
  /** 모든 업무(사이드바 목록과 같은 것 — 같은 시트를 쓰는 다른 업무, 같은 업무명 확인). */
  jobs: JobSummary[];
  /** 이 업무를 마지막으로 볼 때의 단계(처음이면 없음). */
  initialStep?: Step;
  onStep: (s: Step) => void;
  /** 마지막으로 본 채팅 번호(업무마다, 사이드바·모니터링의 안 읽은 수). */
  chatSeen: number;
  onChatSeen: (id: number) => void;
  excelRunning: boolean | null;
  onExcelStatus: (running: boolean) => void;
  /** 새 취합 업무를 만든다('새 취합 시작'). */
  onNewJob: () => void;
  /** 이 업무를 끝낸다(확인 대화상자). */
  onEndJob: () => void;
  /** 이 업무를 에이전트로 넘겼다(이 PC의 업무는 끝났다). */
  onHandedOff: (view: AgentServiceView) => void;
  /**
   * 에이전트로 넘긴 업무(관리자 연결 — 업무 id 자리에 서비스 id). 이 PC 업무처럼 다루되 다시 불러오기·이 PC 공유·저장은
   * 없고, 마지막 단계가 '에이전트'다.
   */
  remote?: AgentServiceView;
  /** 에이전트 업무를 목록에서 지웠다. */
  onForgotten?: () => void;
  /** 사이드바 위쪽(업무 목록). */
  sidebar: ReactNode;
  /** 화면 위 알림(앱을 켜며 되살린 업무 등). */
  banner?: ReactNode;
  toast: (msg: string) => void;
  version: string;
}

/**
 * 업무 하나의 화면: 진행 단계(1a 불러오기 → 1b 설정 → 1c 서비스 → 4 모니터링 → 5 히스토리 → 6 저장). 업무를 바꾸면
 * 새로 그린다(`key` = 업무 id) — 단계는 업무마다 기억하고, 시트·공개 상태·접속자 표·소통은 처음 그릴 때 백엔드에서 읽는다.
 */
export function JobWorkspace(props: Props) {
  const { jobs, onStep, chatSeen, onChatSeen, toast, remote } = props;
  const api = useApi();
  const steps = remote ? AGENT_STEPS : STEPS;
  const [step, setStepState] = useState<Step>(props.initialStep ?? (remote ? "monitor" : "import"));
  const setStep = useCallback(
    (s: Step) => {
      setStepState(s);
      onStep(s);
    },
    [onStep],
  );
  /** 확정한 시트들(세션 안 순번 순서). 비어 있으면 아직 불러오지 않았다. */
  const [sheets, setSheets] = useState<SheetModel[]>([]);
  /** 처음 그릴 때 백엔드에서 읽었다. */
  const [ready, setReady] = useState(false);
  /** 1b·1c·1d·1h에서 함께 보는 시트(세션 안 순번). */
  const [current, setCurrent] = useState(0);
  const loaded = sheets.length > 0;
  // 다시 불러와 확정할 때마다 1b 화면 상태(선택·시점)를 새로 시작한다.
  const [commitId, setCommitId] = useState(0);
  const [service, setService] = useState<ServiceInfo | null>(null);
  /** 이 업무의 주소 경로(공개하지 않았어도 정해 둔 값). */
  const [slug, setSlug] = useState("");
  const [snapshot, setSnapshot] = useState<HostSnapshot | null>(null);
  const [comms, setComms] = useState<CommsSnapshot | null>(null);
  const [footer, setFooter] = useState<FooterOverride | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const frame = useMemo(() => ({ setFooter, toast }), [toast]);

  /** 에이전트 업무는 관리자 연결이 열려야 읽을 수 있다(열리면 다시 읽어 본다). */
  const linked = remote ? remote.linked : true;
  useEffect(() => {
    if (ready || !linked) return;
    void api
      .jobState()
      .then((st) => {
        setSheets(st.models);
        setService(st.service ?? null);
        setSlug(st.slug);
        if (st.snapshot) setSnapshot(st.snapshot);
        if (st.comms) setComms(st.comms);
        // 처음 여는 업무: 시트가 있으면(되살린 업무) 공개 중이면 모니터링, 아니면 서비스로.
        if (!props.initialStep && !remote && st.models.length > 0) setStep(st.service ? "monitor" : "service");
        setLoadError(null);
        setReady(true);
      })
      .catch((e) => setLoadError(errorText(e)));
    // 업무마다 한 번(업무를 바꾸면 이 화면이 새로 그려진다). 에이전트 업무는 연결될 때까지.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [api, linked, ready]);

  // 접속자 표·소통은 백엔드가 바뀔 때마다 밀어 준다(폴링하지 않음).
  useJobListen<HostSnapshot>("participants", setSnapshot);
  useJobListen<CommsSnapshot>("comms", setComms);
  // 마감 동작 '공유 닫기': 백엔드가 공유를 닫은 뒤 알린다. 공유 상태(위쪽 알약·1c·1d)를 다시 읽어 '공유 전'으로 바꾸고,
  // 필요하면 1c에서 다시 공유를 시작한다(같은 마감 시각으로는 다시 닫지 않는다).
  useJobListen<HostEvent>("sheet-changes", (ev) => {
    if (ev.type !== "deadlineClose") return;
    void api
      .serviceInfo()
      .then((info) => {
        setService(info);
        if (!info) toast("마감 시각이 되어 이 업무의 공유를 닫았어요. 필요하면 '서비스'에서 다시 공유를 시작하세요");
      })
      .catch(() => {});
  });

  const onCommitted = useCallback(
    (m: SheetModel[]) => {
      setSheets(m);
      setCurrent(0);
      setCommitId((n) => n + 1);
      setStep("config");
      void api.participantsGet().then((s) => s && setSnapshot(s));
      void api
        .commsGet()
        .then(setComms)
        .catch(() => {});
    },
    [api, setStep],
  );

  const unreadChat = (comms?.chat ?? []).filter((m) => !m.host && m.id > chatSeen).length;
  const canGo = (s: Step) => !NOT_YET.includes(s) && (remote ? s !== "import" && loaded : s === "import" || loaded);
  /** 참여자가 접속할 수 있다(이 PC 공유 중, 또는 에이전트가 서비스 중). */
  const running = remote ? remote.status?.state === "running" : service !== null;

  const index = steps.findIndex((s) => s.id === step);
  const cur = steps[index]!;
  const prevStep = steps[index - 1];
  const nextStep = steps[index + 1];
  const defaultHint =
    step === "config"
      ? "설정은 공유 중에도 바꿀 수 있어요"
      : step === "service" && !running
        ? "공유를 시작해야 참여자가 접속할 수 있어요"
        : "";
  const hint = footer?.hint ?? defaultHint;
  const goNext = footer?.onNext ?? (() => nextStep && setStep(nextStep.id));
  const nextDisabled = footer?.nextDisabled ?? (nextStep ? !canGo(nextStep.id) : true);
  const published = jobs.filter((j) => j.published).length;

  return (
    <FrameContext.Provider value={frame}>
      <div className="shell">
        <header className="titlebar">
          <div className="brand">
            <span className="logo" />
            <b>Merge Master</b>
          </div>
          <span className="divider" />
          <span className="file">
            {!loaded
              ? remote
                ? `${remote.workbook} · 에이전트 ${remote.agentName}`
                : "열린 파일 없음"
              : sheets.length === 1
                ? `${sheets[0]!.workbook} · ${sheets[0]!.sheet}`
                : `${sheets[0]!.workbook} · 시트 ${sheets.length}개`}
            {loaded && remote && ` · 에이전트 ${remote.agentName}`}
          </span>
          <span className="spacer" />
          {props.excelRunning !== null && !remote && (
            <span className={`excel ${props.excelRunning ? "ok" : "muted"}`}>
              {props.excelRunning ? "● Excel 연결됨 (COM)" : "○ 실행 중인 Excel 없음"}
            </span>
          )}
          <div className={`service-badge${running ? " on" : ""}`} aria-live="polite">
            {remote
              ? `에이전트 · ${agentStateLabel(remote)}${running ? ` · 접속 ${snapshot?.online ?? 0} / ${snapshot?.roster ?? 0}` : ""}`
              : service
                ? `공유 중 · 접속 ${snapshot?.online ?? 0} / ${snapshot?.roster ?? 0}`
                : "공유 전"}
            {snapshot?.closed && <span className="warn"> · 취합 마감됨</span>}
            {!remote && published > (service ? 1 : 0) && (
              <span className="muted" title="같은 포트로 공유 중인 다른 업무">
                {" "}
                · 다른 업무 {published - (service ? 1 : 0)}개 공유 중
              </span>
            )}
          </div>
        </header>
        <div className="body">
          <nav className="steps" aria-label="진행 단계">
            {props.sidebar}
            <div className="steps-title">진행 단계</div>
            {steps.map((s, i) => {
              const done = i < index || (s.id === "import" && loaded && step !== "import");
              return (
                <button
                  key={s.id}
                  className={`step${s.id === step ? " active" : done ? " done" : ""}`}
                  aria-label={`${i + 1} ${s.label}`}
                  aria-current={s.id === step ? "step" : undefined}
                  disabled={!canGo(s.id)}
                  title={
                    remote && s.id === "import"
                      ? "에이전트로 넘긴 업무는 다시 불러올 수 없어요"
                      : NOT_YET.includes(s.id)
                        ? "다음 단계에서 구현"
                        : undefined
                  }
                  onClick={() => setStep(s.id)}
                >
                  <span className="mark">{done ? "✓" : i + 1}</span>
                  <span>{s.label}</span>
                  {s.id === "monitor" && unreadChat > 0 && step !== "monitor" ? (
                    <span className="badge">{unreadChat}</span>
                  ) : null}
                </button>
              );
            })}
            <span className="spacer" />
            {props.version && <div className="app-version">v{props.version}</div>}
          </nav>
          <div className="main">
            <div className="step-head">
              <div>
                <div className="step-no">
                  {index + 1} / {steps.length} 단계{remote ? " · 에이전트에서 진행 중" : ""}
                </div>
                <h1>{cur.label}</h1>
                <p>{cur.desc}</p>
              </div>
            </div>
            <main className="content">
              {props.banner}
              {remote &&
                !remote.linked &&
                (ready ? (
                  <div className="warn small" role="status">
                    에이전트에 연결할 수 없어요(다시 연결하는 중) — 보이는 내용은 마지막으로 받은 것이에요.
                    {remote.problem ? ` ${remote.problem}` : ""}
                  </div>
                ) : (
                  <div className="muted small" role="status">
                    에이전트({remote.agentName})에 연결하는 중…
                  </div>
                ))}
              {loadError && (!remote || remote.linked) && <div className="error small">{loadError}</div>}
              {ready && (
                <>
                  {/* 불러오기·설정 화면은 단계를 옮겨도 상태를 잃지 않도록 숨기기만 한다. */}
                  {!remote && (
                    <div className="screen-slot" hidden={step !== "import"}>
                      <ImportScreen
                        active={step === "import"}
                        committed={loaded}
                        jobs={jobs}
                        onExcelStatus={props.onExcelStatus}
                        onCommitted={onCommitted}
                        onNext={() => setStep("config")}
                      />
                    </div>
                  )}
                  {loaded && (
                    <div className="screen-slot" hidden={step !== "config"}>
                      <ConfigScreen
                        key={commitId}
                        models={sheets}
                        sheet={current}
                        onSheet={setCurrent}
                        active={step === "config"}
                      />
                    </div>
                  )}
                  {step === "service" && (
                    <ServiceScreen
                      active
                      hasSheet={loaded}
                      service={service}
                      onService={setService}
                      slug={slug}
                      jobs={jobs}
                      snapshot={snapshot}
                      sheets={sheets}
                      onGoConfig={() => setStep("config")}
                      onHandedOff={props.onHandedOff}
                      remote={remote}
                    />
                  )}
                  {step === "monitor" && (
                    <MonitorScreen
                      active
                      snapshot={snapshot}
                      comms={comms}
                      running={running}
                      chatSeen={chatSeen}
                      onChatSeen={onChatSeen}
                      sheets={sheets}
                      sheet={current}
                      onSheet={setCurrent}
                      onGoHistory={() => setStep("history")}
                    />
                  )}
                  {step === "history" && <HistoryScreen active sheets={sheets} sheet={current} />}
                  {step === "agent" && remote && (
                    <AgentPanel view={remote} toast={toast} onForgotten={props.onForgotten ?? (() => {})} />
                  )}
                  {step === "save" && !remote && (
                    <SaveScreen
                      active
                      snapshot={snapshot}
                      onStopped={() => setService(null)}
                      onRestart={props.onNewJob}
                      onEndJob={props.onEndJob}
                    />
                  )}
                </>
              )}
            </main>
            <footer className="step-foot">
              <div>
                {prevStep && (
                  <button className="ghost" onClick={() => setStep(prevStep.id)}>
                    ← {prevStep.label}
                  </button>
                )}
              </div>
              <div className="next">
                {hint && <span className="hint">{hint}</span>}
                {nextStep && (
                  <button disabled={nextDisabled} onClick={goNext}>
                    다음: {nextStep.label} →
                  </button>
                )}
              </div>
            </footer>
          </div>
        </div>
      </div>
    </FrameContext.Provider>
  );
}
