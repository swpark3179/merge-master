import type { CommsSnapshot, HostSnapshot, RecoveryInfo, ServiceInfo, SheetModel } from "@mm/shared";
import { listen } from "@tauri-apps/api/event";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import { getVersion } from "@tauri-apps/api/app";

import { api, errorText } from "./api";
import type { FooterOverride } from "./frame";
import { FrameContext } from "./frame";
import { ConfigScreen } from "./screens/config/ConfigScreen";
import { HistoryScreen } from "./screens/history/HistoryScreen";
import { ImportScreen } from "./screens/import/ImportScreen";
import { MonitorScreen } from "./screens/monitor/MonitorScreen";
import { SaveScreen } from "./screens/save/SaveScreen";
import { ServiceScreen } from "./screens/service/ServiceScreen";

type Step = "import" | "config" | "service" | "monitor" | "history" | "save";

const STEPS: { id: Step; label: string; desc: string }[] = [
  { id: "import", label: "불러오기", desc: "참여자와 함께 채울 엑셀 파일을 고르세요." },
  { id: "config", label: "설정", desc: "참여자가 어느 칸을 채울지 정하세요." },
  { id: "service", label: "서비스", desc: "주소를 공유하면 참여자가 브라우저로 접속합니다." },
  { id: "monitor", label: "모니터링", desc: "참여자가 입력하는 내용을 실시간으로 확인하세요." },
  { id: "history", label: "히스토리", desc: "누가 언제 무엇을 바꿨는지 확인하세요." },
  { id: "save", label: "저장", desc: "모은 내용을 엑셀 파일로 저장하세요." },
];

/** 아직 만들지 않은 단계. */
const NOT_YET: Step[] = [];

export function App() {
  const [step, setStep] = useState<Step>("import");
  /** 확정한 시트들(세션 안 순번 순서). 비어 있으면 아직 불러오지 않았다. */
  const [sheets, setSheets] = useState<SheetModel[]>([]);
  /** 1b·1c·1d·1h에서 함께 보는 시트(세션 안 순번). */
  const [current, setCurrent] = useState(0);
  const loaded = sheets.length > 0;
  // 다시 불러와 확정할 때마다 1b 화면 상태(선택·시점)를 새로 시작한다.
  const [commitId, setCommitId] = useState(0);
  const [service, setService] = useState<ServiceInfo | null>(null);
  /** 앱을 다시 켰을 때 이어서 할 수 있는 지난 취합. */
  const [recovery, setRecovery] = useState<RecoveryInfo | null>(null);
  const [recoveryBusy, setRecoveryBusy] = useState(false);
  const [recoveryError, setRecoveryError] = useState<string | null>(null);

  const [version, setVersion] = useState("");
  const [footer, setFooter] = useState<FooterOverride | null>(null);
  const [toastText, setToastText] = useState("");
  const toastTimer = useRef(0);
  const frame = useMemo(
    () => ({
      setFooter,
      toast: (msg: string) => {
        setToastText(msg);
        window.clearTimeout(toastTimer.current);
        toastTimer.current = window.setTimeout(() => setToastText(""), 2200);
      },
    }),
    [],
  );

  useEffect(() => {
    void api
      .recoveryCheck()
      .then(setRecovery)
      .catch(() => {});
    void getVersion()
      .then(setVersion)
      .catch(() => {});
  }, []);
  const [snapshot, setSnapshot] = useState<HostSnapshot | null>(null);
  const [comms, setComms] = useState<CommsSnapshot | null>(null);
  /** 마지막으로 본 채팅 번호(사이드바·모니터링의 안 읽은 수). */
  const [chatSeen, setChatSeen] = useState(0);
  const [excelRunning, setExcelRunning] = useState<boolean | null>(null);

  // 접속자 표는 서버가 바뀔 때마다 밀어 준다(폴링하지 않음).
  useEffect(() => {
    const off = listen<HostSnapshot>("participants", (e) => setSnapshot(e.payload));
    const offComms = listen<CommsSnapshot>("comms", (e) => setComms(e.payload));
    return () => {
      void off.then((f) => f());
      void offComms.then((f) => f());
    };
  }, []);

  // 세션은 시트를 확정할 때 생기고 서비스를 멈춰도 남는다(진행률·명단은 계속 보인다).
  const resume = async () => {
    setRecoveryBusy(true);
    setRecoveryError(null);
    try {
      const r = await api.recoveryResume();
      setRecovery(null);
      setSheets(r.models);
      setCurrent(0);
      setCommitId((n) => n + 1);
      setService(r.service ?? null);
      void api.participantsGet().then((s) => s && setSnapshot(s));
      void api
        .commsGet()
        .then(setComms)
        .catch(() => {});
      if (r.serviceError) setRecoveryError(`취합은 이어서 했지만 서비스를 다시 열지 못했습니다: ${r.serviceError}`);
      setStep(r.service ? "monitor" : "service");
    } catch (e) {
      setRecoveryError(errorText(e));
    } finally {
      setRecoveryBusy(false);
    }
  };

  const discard = async () => {
    await api.recoveryDiscard().catch(() => {});
    setRecovery(null);
  };

  const onCommitted = useCallback((m: SheetModel[]) => {
    // 새 취합을 시작했으므로 지난 취합은 더 묻지 않는다(파일은 남는다).
    setRecovery(null);
    setSheets(m);
    setCurrent(0);
    setCommitId((n) => n + 1);
    setStep("config");
    void api.participantsGet().then((s) => s && setSnapshot(s));
    void api
      .commsGet()
      .then(setComms)
      .catch(() => {});
  }, []);

  const onChatSeen = useCallback((id: number) => setChatSeen((s) => Math.max(s, id)), []);
  const unreadChat = (comms?.chat ?? []).filter((m) => !m.host && m.id > chatSeen).length;

  const ready = (s: Step) => !NOT_YET.includes(s) && (s === "import" || loaded);

  const index = STEPS.findIndex((s) => s.id === step);
  const cur = STEPS[index]!;
  const prevStep = STEPS[index - 1];
  const nextStep = STEPS[index + 1];
  const defaultHint =
    step === "config"
      ? "설정은 공유 중에도 바꿀 수 있어요"
      : step === "service" && !service
        ? "공유를 시작해야 참여자가 접속할 수 있어요"
        : "";
  const hint = footer?.hint ?? defaultHint;
  const goNext = footer?.onNext ?? (() => nextStep && setStep(nextStep.id));
  const nextDisabled = footer?.nextDisabled ?? (nextStep ? !ready(nextStep.id) : true);

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
              ? "열린 파일 없음"
              : sheets.length === 1
                ? `${sheets[0]!.workbook} · ${sheets[0]!.sheet}`
                : `${sheets[0]!.workbook} · 시트 ${sheets.length}개`}
          </span>
          <span className="spacer" />
          {excelRunning !== null && (
            <span className={`excel ${excelRunning ? "ok" : "muted"}`}>
              {excelRunning ? "● Excel 연결됨 (COM)" : "○ 실행 중인 Excel 없음"}
            </span>
          )}
          <div className={`service-badge${service ? " on" : ""}`} aria-live="polite">
            {service ? `공유 중 · 접속 ${snapshot?.online ?? 0} / ${snapshot?.roster ?? 0}` : "공유 전"}
            {snapshot?.closed && <span className="warn"> · 취합 마감됨</span>}
          </div>
        </header>
        <div className="body">
          <nav className="steps" aria-label="진행 단계">
            <div className="steps-title">진행 단계</div>
            {STEPS.map((s, i) => {
              const done = i < index || (s.id === "import" && loaded && step !== "import");
              return (
                <button
                  key={s.id}
                  className={`step${s.id === step ? " active" : done ? " done" : ""}`}
                  aria-label={`${i + 1} ${s.label}`}
                  aria-current={s.id === step ? "step" : undefined}
                  disabled={!ready(s.id)}
                  title={NOT_YET.includes(s.id) ? "다음 단계에서 구현" : undefined}
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
            {version && <div className="app-version">v{version}</div>}
          </nav>
          <div className="main">
            <div className="step-head">
              <div>
                <div className="step-no">{index + 1} / 6 단계</div>
                <h1>{cur.label}</h1>
                <p>{cur.desc}</p>
              </div>
            </div>
            <main className="content">
              {(recovery || recoveryError) && (
                <div className="recovery-card" role="region" aria-label="지난 취합">
                  {recovery && (
                    <>
                      <b>지난 취합을 이어서 할 수 있습니다</b>
                      <span>
                        {recovery.workbook} · {recovery.sheets.join(", ")} · 변경 {recovery.changes.toLocaleString()}건
                        {recovery.lastChangeMs
                          ? ` · 마지막 변경 ${new Date(recovery.lastChangeMs).toLocaleString("ko-KR")}`
                          : ""}
                        {recovery.port ? ` · 서비스 포트 ${recovery.port}(다시 엽니다)` : ""}
                        {recovery.closed ? " · 마감됨" : ""}
                      </span>
                      <span className="spacer" />
                      <button disabled={recoveryBusy} onClick={() => void resume()}>
                        {recoveryBusy ? "되살리는 중…" : "이어서 하기"}
                      </button>
                      <button className="ghost" disabled={recoveryBusy} onClick={() => void discard()}>
                        버리고 새로 시작
                      </button>
                    </>
                  )}
                  {recoveryError && <span className="error small">{recoveryError}</span>}
                  {!recovery && (
                    <button className="ghost small" onClick={() => setRecoveryError(null)}>
                      닫기
                    </button>
                  )}
                </div>
              )}
              {/* 불러오기·설정 화면은 단계를 옮겨도 상태를 잃지 않도록 숨기기만 한다. */}
              <div className="screen-slot" hidden={step !== "import"}>
                <ImportScreen
                  active={step === "import"}
                  committed={loaded}
                  onExcelStatus={setExcelRunning}
                  onCommitted={onCommitted}
                  onNext={() => setStep("config")}
                />
              </div>
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
                  snapshot={snapshot}
                  sheets={sheets}
                  onGoConfig={() => setStep("config")}
                />
              )}
              {step === "monitor" && (
                <MonitorScreen
                  active
                  snapshot={snapshot}
                  comms={comms}
                  running={service !== null}
                  chatSeen={chatSeen}
                  onChatSeen={onChatSeen}
                  sheets={sheets}
                  sheet={current}
                  onSheet={setCurrent}
                  onGoHistory={() => setStep("history")}
                />
              )}
              {step === "history" && <HistoryScreen active sheets={sheets} sheet={current} />}
              {step === "save" && (
                <SaveScreen
                  active
                  snapshot={snapshot}
                  onStopped={() => setService(null)}
                  onRestart={() => setStep("import")}
                />
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
        {toastText && <div className="toast-host">{toastText}</div>}
      </div>
    </FrameContext.Provider>
  );
}
