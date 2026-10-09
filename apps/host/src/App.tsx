import type { CommsSnapshot, HostSnapshot, RecoveryInfo, ServiceInfo, SheetModel } from "@mm/shared";
import { listen } from "@tauri-apps/api/event";
import { useCallback, useEffect, useState } from "react";

import { getVersion } from "@tauri-apps/api/app";

import { api, errorText } from "./api";
import { ConfigScreen } from "./screens/config/ConfigScreen";
import { HistoryScreen } from "./screens/history/HistoryScreen";
import { ImportScreen } from "./screens/import/ImportScreen";
import { MonitorScreen } from "./screens/monitor/MonitorScreen";
import { SaveScreen } from "./screens/save/SaveScreen";
import { ServiceScreen } from "./screens/service/ServiceScreen";

type Step = "import" | "config" | "service" | "monitor" | "history" | "save";

const STEPS: { id: Step; label: string }[] = [
  { id: "import", label: "1 불러오기" },
  { id: "config", label: "2 취합 설정" },
  { id: "service", label: "3 서비스·접속자" },
  { id: "monitor", label: "4 진행 모니터링" },
  { id: "history", label: "5 히스토리" },
  { id: "save", label: "6 저장·마감" },
];

/** 아직 만들지 않은 단계. */
const NOT_YET: Step[] = [];

export function App() {
  const [step, setStep] = useState<Step>("import");
  const [sheet, setSheet] = useState<SheetModel | null>(null);
  // 다시 불러와 확정할 때마다 1b 화면 상태(선택·시점)를 새로 시작한다.
  const [commitId, setCommitId] = useState(0);
  const [service, setService] = useState<ServiceInfo | null>(null);
  /** 앱을 다시 켰을 때 이어서 할 수 있는 지난 취합. */
  const [recovery, setRecovery] = useState<RecoveryInfo | null>(null);
  const [recoveryBusy, setRecoveryBusy] = useState(false);
  const [recoveryError, setRecoveryError] = useState<string | null>(null);

  const [version, setVersion] = useState("");

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
      setSheet(r.model);
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

  const onCommitted = useCallback((m: SheetModel) => {
    // 새 취합을 시작했으므로 지난 취합은 더 묻지 않는다(파일은 남는다).
    setRecovery(null);
    setSheet(m);
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

  const ready = (s: Step) => !NOT_YET.includes(s) && (s === "import" || sheet !== null);

  return (
    <div className="shell">
      <header className="titlebar">
        <span className="logo" />
        <b>엑셀 취합 도구</b>
        <span className="muted">{sheet ? `${sheet.workbook} · ${sheet.sheet}` : "새 취합"}</span>
        <span className="spacer" />
        {excelRunning !== null && (
          <span className={excelRunning ? "ok" : "muted"}>
            {excelRunning ? "● Excel 연결됨 (COM)" : "○ 실행 중인 Excel 없음"}
          </span>
        )}
      </header>
      <div className="body">
        <nav className="steps">
          {STEPS.map((s) => (
            <button
              key={s.id}
              className={`step${s.id === step ? " active" : ""}`}
              disabled={!ready(s.id)}
              title={NOT_YET.includes(s.id) ? "다음 단계에서 구현" : undefined}
              onClick={() => setStep(s.id)}
            >
              {s.label}
              {s.id === "import" && sheet ? " ✓" : ""}
              {s.id === "monitor" && unreadChat > 0 && step !== "monitor" ? (
                <span className="badge">{unreadChat}</span>
              ) : null}
            </button>
          ))}
          <span className="spacer" />
          <div className={`service-badge${service ? " on" : ""}`}>
            {service ? (
              <>
                ● 실행 중
                <br />
                접속 {snapshot?.online ?? 0} / {snapshot?.roster ?? 0}
              </>
            ) : (
              "○ 서비스 중지됨"
            )}
            {snapshot?.closed && (
              <>
                <br />
                <span className="warn">취합 마감됨</span>
              </>
            )}
          </div>
          {version && <div className="app-version">v{version}</div>}
        </nav>
        <main className="content">
          {(recovery || recoveryError) && (
            <div className="recovery-card" role="region" aria-label="지난 취합">
              {recovery && (
                <>
                  <b>지난 취합을 이어서 할 수 있습니다</b>
                  <span>
                    {recovery.workbook} · {recovery.sheet} · 변경 {recovery.changes.toLocaleString()}건
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
            <ImportScreen onExcelStatus={setExcelRunning} onCommitted={onCommitted} />
          </div>
          {sheet && (
            <div className="screen-slot" hidden={step !== "config"}>
              <ConfigScreen
                key={commitId}
                model={sheet}
                active={step === "config"}
                onBack={() => setStep("import")}
                onNext={() => setStep("service")}
              />
            </div>
          )}
          {step === "service" && (
            <ServiceScreen
              active
              hasSheet={sheet !== null}
              service={service}
              onService={setService}
              snapshot={snapshot}
              origin={sheet?.origin ?? { row: 0, col: 0 }}
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
              onGoHistory={() => setStep("history")}
            />
          )}
          {step === "history" && <HistoryScreen active />}
          {step === "save" && <SaveScreen active snapshot={snapshot} onStopped={() => setService(null)} />}
        </main>
      </div>
    </div>
  );
}
