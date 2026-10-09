import type { CollectConfig, FirewallStatus, HostSnapshot, LanAddress, RecalcStatus, ServiceInfo } from "@mm/shared";
import { formatDeadline } from "@mm/shared";
import { listen } from "@tauri-apps/api/event";
import { useCallback, useEffect, useState } from "react";

import { api, errorText } from "../../api";
import { ParticipantsPanel } from "./ParticipantsPanel";

interface Props {
  active: boolean;
  hasSheet: boolean;
  service: ServiceInfo | null;
  onService: (s: ServiceInfo | null) => void;
  snapshot: HostSnapshot | null;
  /** 시트 기준 좌표 변환용(현재 위치 표시). */
  origin: { row: number; col: number };
  onGoConfig: () => void;
}

/** 1c 서비스 시작 · 접속자 관리. */
export function ServiceScreen({ active, hasSheet, service, onService, snapshot, origin, onGoConfig }: Props) {
  const [config, setConfig] = useState<CollectConfig | null>(null);
  const [roster, setRoster] = useState<string[]>([]);
  const [lans, setLans] = useState<LanAddress[]>([]);
  const [ip, setIp] = useState<string | null>(null);
  // 단계를 오가면 이 화면은 새로 그려진다: 실행 중이면 실제 포트·경로를 보인다.
  const [port, setPort] = useState(() => String(service?.port ?? 8080));
  const [slug, setSlug] = useState(() => service?.slug ?? "q4");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [copied, setCopied] = useState<string | null>(null);
  const [qr, setQr] = useState<string | null>(null);
  const [firewall, setFirewall] = useState<FirewallStatus | "checking" | null>(null);
  const [recalc, setRecalc] = useState<RecalcStatus | null>(null);

  // 수식 재계산(Excel 계산 사본) 상태
  useEffect(() => {
    void api.recalcStatus().then(setRecalc);
    const off = listen<RecalcStatus>("recalc-status", (e) => setRecalc(e.payload));
    return () => void off.then((f) => f());
  }, []);

  const reload = useCallback(async () => {
    if (!hasSheet) return;
    try {
      const v = await api.configView(null);
      setConfig(v.config);
      setRoster(v.roster);
    } catch (e) {
      setError(errorText(e));
    }
  }, [hasSheet]);

  useEffect(() => {
    if (!active) return;
    void reload();
    void api.lanAddresses().then((l) => {
      setLans(l);
      setIp((cur) => cur ?? l[0]?.ip ?? "127.0.0.1");
    });
  }, [active, reload]);

  const url = service ? `http://${ip ?? service.urls[0]?.ip ?? "127.0.0.1"}:${service.port}/${service.slug}` : null;
  useEffect(() => {
    if (!url) return setQr(null);
    void api.qrSvg(url).then(setQr);
  }, [url]);

  const saveMeta = async (patch: Partial<Pick<CollectConfig, "title" | "description" | "requester" | "deadline">>) => {
    if (!config) return;
    const next = { ...config, ...patch };
    try {
      const v = await api.configApply(
        {
          op: "setMeta",
          title: next.title,
          description: next.description,
          requester: next.requester,
          deadline: next.deadline,
        },
        null,
      );
      setConfig(v.config);
    } catch (e) {
      setError(errorText(e));
    }
  };

  const setLock = async (on: boolean) => {
    if (!config) return;
    const v = await api.configApply(
      { op: "setPermissions", permissions: { ...config.permissions, lockAfterDeadline: on } },
      null,
    );
    setConfig(v.config);
  };

  const start = async () => {
    setBusy(true);
    setError(null);
    try {
      onService(await api.serverStart(Number(port), slug.trim() || "q4"));
    } catch (e) {
      setError(errorText(e));
    } finally {
      setBusy(false);
    }
  };

  const stop = async () => {
    setBusy(true);
    try {
      await api.serverStop();
      onService(null);
    } finally {
      setBusy(false);
    }
  };

  const copy = async (what: string, text: string) => {
    await navigator.clipboard.writeText(text);
    setCopied(what);
    window.setTimeout(() => setCopied(null), 1500);
  };

  const notice = () =>
    [
      `[${config?.title || "엑셀 취합"}] 취합 요청${config?.requester ? ` · ${config.requester}` : ""}`,
      config?.description || "",
      config?.deadline ? `마감: ${formatDeadline(config.deadline)}` : "",
      `접속: ${url}`,
      "같은 사내망에서 위 주소를 열고 이름만 입력하면 됩니다.",
    ]
      .filter(Boolean)
      .join("\n");

  const checkFirewall = async () => {
    setFirewall("checking");
    try {
      setFirewall(await api.firewallCheck(Number(port)));
    } catch (e) {
      setFirewall(null);
      setError(errorText(e));
    }
  };

  return (
    <div className="screen service-screen">
      <section className="card service" aria-label="웹소켓 서비스">
        <div className="card-head">
          <b className="big">웹소켓 서비스</b>
          <span className="spacer" />
          {service ? <span className="ok">● 실행 중</span> : <span className="muted">○ 중지됨</span>}
        </div>
        {!hasSheet && <div className="warn small">먼저 시트를 불러와 확정해 주세요(1 불러오기).</div>}
        {config && (
          <>
            <label className="field">
              취합 제목
              <input
                aria-label="취합 제목"
                defaultValue={config.title}
                key={`t${config.title}`}
                onBlur={(e) => e.target.value !== config.title && void saveMeta({ title: e.target.value })}
              />
            </label>
            <label className="field">
              요청자
              <input
                aria-label="요청자"
                placeholder="예: 박지훈 (기획팀)"
                defaultValue={config.requester}
                key={`r${config.requester}`}
                onBlur={(e) => e.target.value !== config.requester && void saveMeta({ requester: e.target.value })}
              />
            </label>
            <label className="field">
              안내
              <textarea
                aria-label="안내"
                rows={2}
                placeholder="참여자 접속 화면에 보일 안내"
                defaultValue={config.description}
                key={`d${config.description}`}
                onBlur={(e) => e.target.value !== config.description && void saveMeta({ description: e.target.value })}
              />
            </label>
          </>
        )}
        <div className="field">
          네트워크
          {lans.length === 0 && <span className="warn small">사용 가능한 네트워크 주소가 없습니다.</span>}
          {lans.map((l) => (
            <label key={l.ip} className="radio">
              <input type="radio" name="lan" checked={ip === l.ip} onChange={() => setIp(l.ip)} />
              {l.name} · {l.ip}
            </label>
          ))}
        </div>
        <div className="row">
          <label className="field">
            포트
            <input
              aria-label="포트"
              className="port"
              value={port}
              onChange={(e) => setPort(e.target.value.replace(/\D/g, ""))}
              disabled={!!service}
            />
          </label>
          <label className="field">
            주소 경로
            <input
              aria-label="주소 경로"
              className="port"
              value={slug}
              onChange={(e) => setSlug(e.target.value.replace(/[^\w-]/g, ""))}
              disabled={!!service}
            />
          </label>
          {config && (
            <label className="field grow">
              마감
              <input
                aria-label="마감"
                type="datetime-local"
                value={config.deadline ?? ""}
                onChange={(e) => void saveMeta({ deadline: e.target.value || undefined })}
              />
            </label>
          )}
        </div>
        {service && url && (
          <div className="url-block">
            <div className="url-main">
              <span className="muted small">접속 주소</span>
              <code className="url">{url}</code>
              <div className="row">
                <button className="ghost small" onClick={() => void copy("url", url)}>
                  {copied === "url" ? "복사됨" : "주소 복사"}
                </button>
                <button className="ghost small" onClick={() => void copy("notice", notice())}>
                  {copied === "notice" ? "복사됨" : "안내문 복사"}
                </button>
              </div>
            </div>
            {qr && <div className="qr" aria-label="접속 주소 QR" dangerouslySetInnerHTML={{ __html: qr }} />}
          </div>
        )}
        <div className="small">
          <span className="muted">참여 방식</span> 이름만 입력 · 명단 외 이름은 승인 후 배정
        </div>
        {recalc && recalc.state !== "none" && (
          <div className="small" aria-label="수식 계산">
            <span className="muted">수식 계산</span>{" "}
            {recalc.state === "opening" ? (
              <span className="muted">Excel 계산 사본을 여는 중…</span>
            ) : recalc.state === "ready" ? (
              <span className="ok">● Excel이 바로 계산 (수식 {recalc.formulas.toLocaleString()}칸)</span>
            ) : (
              <span className="warn" title={recalc.message}>
                ○ 저장할 때 계산
              </span>
            )}
          </div>
        )}
        {config && (
          <label className="check">
            <input
              type="checkbox"
              checked={config.permissions.lockAfterDeadline}
              onChange={(e) => void setLock(e.target.checked)}
            />
            마감 후 자동 읽기 전용
          </label>
        )}
        <div className="note small">
          다른 PC에서 접속이 안 되면 Windows 방화벽을 확인하세요.{" "}
          <button className="link" onClick={() => void checkFirewall()} disabled={firewall === "checking"}>
            {firewall === "checking" ? "확인 중…" : "방화벽 확인"}
          </button>
          {firewall && firewall !== "checking" && (
            <div className="firewall">
              {firewall.appBlocked
                ? "이 앱을 막는 규칙이 있습니다(처음 물어볼 때 취소함)."
                : firewall.appAllowed || firewall.portAllowed
                  ? "✓ 허용 규칙이 있습니다."
                  : "허용 규칙이 없습니다. 처음 서비스를 시작할 때 Windows가 물으면 허용하거나, 아래에서 규칙을 추가하세요."}
              {!(firewall.appAllowed || firewall.portAllowed) && (
                <button
                  className="ghost small"
                  onClick={() =>
                    void api
                      .firewallAdd(Number(port))
                      .then(checkFirewall)
                      .catch((e) => setError(errorText(e)))
                  }
                >
                  포트 {port} 허용 규칙 추가(관리자 권한)
                </button>
              )}
            </div>
          )}
        </div>
        {error && <div className="error small">{error}</div>}
        <span className="spacer" />
        {service ? (
          <button className="danger" onClick={() => void stop()} disabled={busy}>
            서비스 중지
          </button>
        ) : (
          <button onClick={() => void start()} disabled={busy || !port || !hasSheet}>
            서비스 시작
          </button>
        )}
      </section>

      <ParticipantsPanel
        running={service !== null}
        snapshot={snapshot}
        roster={roster}
        origin={origin}
        onRosterChanged={reload}
        onGoConfig={onGoConfig}
        onError={setError}
      />
    </div>
  );
}
