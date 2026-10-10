import type { CollectConfig, HostSnapshot, LanAddress, RecalcStatus, ServiceInfo } from "@mm/shared";
import { formatDeadline } from "@mm/shared";
import { listen } from "@tauri-apps/api/event";
import { useCallback, useEffect, useState } from "react";

import { api, errorText } from "../../api";
import { useToast } from "../../frame";
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
  const [net, setNet] = useState(false);
  /** 이 화면에서 공유를 시작한 시각(표시용). */
  const [since, setSince] = useState<string | null>(null);
  const toast = useToast();
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
      setSince(new Date().toTimeString().slice(0, 5));
      toast("공유를 시작했어요");
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
      setSince(null);
      toast("공유를 멈췄어요");
    } finally {
      setBusy(false);
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

  const host = ip ?? "127.0.0.1";
  const preview = `http://${host}:${port || "8080"}/${slug.trim() || "q4"}`;
  const subnet = host.split(".").slice(0, 3).join(".");
  const lanName = lans.find((l) => l.ip === host)?.name;

  return (
    <div className="screen service-screen">
      <div className="service-top">
        <section className="card conditions" aria-label="접속 조건">
          <div className="card-title">접속 조건</div>
          {!hasSheet && <div className="warn small">먼저 시트를 불러와 확정해 주세요(1 불러오기).</div>}
          {config && (
            <>
              <div className="two">
                <label className="field">
                  <b>취합 제목</b>
                  <input
                    aria-label="취합 제목"
                    defaultValue={config.title}
                    key={`t${config.title}`}
                    onBlur={(e) => e.target.value !== config.title && void saveMeta({ title: e.target.value })}
                  />
                </label>
                <label className="field">
                  <span>
                    <b>요청자</b> <span className="muted">(선택)</span>
                  </span>
                  <input
                    aria-label="요청자"
                    placeholder="예: 박지훈 (기획팀)"
                    defaultValue={config.requester}
                    key={`r${config.requester}`}
                    onBlur={(e) => e.target.value !== config.requester && void saveMeta({ requester: e.target.value })}
                  />
                </label>
              </div>
              <label className="field">
                <span>
                  <b>안내</b> <span className="muted">(선택)</span>
                </span>
                <textarea
                  aria-label="안내"
                  rows={2}
                  placeholder="참여자 접속 화면에 보일 안내"
                  defaultValue={config.description}
                  key={`d${config.description}`}
                  onBlur={(e) =>
                    e.target.value !== config.description && void saveMeta({ description: e.target.value })
                  }
                />
              </label>
              <div className="two">
                <label className="field">
                  <span>
                    <b>입력 마감</b> <span className="muted">(선택)</span>
                  </span>
                  <input
                    aria-label="마감"
                    type="datetime-local"
                    value={config.deadline ?? ""}
                    onChange={(e) => void saveMeta({ deadline: e.target.value || undefined })}
                  />
                </label>
                <label className="check deadline-lock">
                  <input
                    type="checkbox"
                    checked={config.permissions.lockAfterDeadline}
                    onChange={(e) => void setLock(e.target.checked)}
                  />
                  마감 후 자동 읽기 전용
                </label>
              </div>
            </>
          )}
          <div className="divider" />
          <button className="net-toggle" aria-expanded={net} onClick={() => setNet((v) => !v)}>
            <b>네트워크 설정</b>
            <span className="tag">자동</span>
            <span className="spacer" />
            <span className="muted small">{net ? "접기" : "펼치기"}</span>
          </button>
          {net && (
            <div className="net-fields">
              <label className="field">
                <span>사용할 네트워크</span>
                {lans.length === 0 ? (
                  <span className="warn small">사용 가능한 네트워크 주소가 없습니다.</span>
                ) : (
                  <select aria-label="네트워크" value={host} onChange={(e) => setIp(e.target.value)}>
                    {lans.map((l) => (
                      <option key={l.ip} value={l.ip}>
                        {l.name} · {l.ip}
                      </option>
                    ))}
                  </select>
                )}
              </label>
              <label className="field">
                <span>포트</span>
                <input
                  aria-label="포트"
                  value={port}
                  onChange={(e) => setPort(e.target.value.replace(/\D/g, ""))}
                  disabled={!!service}
                />
              </label>
              <label className="field">
                <span>주소 경로</span>
                <input
                  aria-label="주소 경로"
                  value={slug}
                  onChange={(e) => setSlug(e.target.value.replace(/[^\w-]/g, ""))}
                  disabled={!!service}
                />
              </label>
            </div>
          )}
          <div className="reach">
            <span className="dot-ok" />
            같은 {lanName ? `${lanName.split(" ")[0]}` : "사내망"}({subnet}.x)에 있는 사람은 바로 접속할 수 있어요.
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
        </section>

        <section className="card share" aria-label="공유">
          {service && url ? (
            <>
              <div className="card-head">
                <span className="card-title">참여자에게 보낼 주소</span>
                <span className="spacer" />
                <span className="pill ok-pill">공유 중{since ? ` · ${since}부터` : ""}</span>
              </div>
              <div className="url-row">
                <code className="url">{url}</code>
                <button onClick={() => void copy(url, "주소를 복사했어요")}>복사</button>
              </div>
              <div className="row">
                <button className="ghost grow-btn" onClick={() => void copy(notice(), "안내 문구를 복사했어요")}>
                  안내 문구 복사
                </button>
                <button className="danger grow-btn" onClick={() => void stop()} disabled={busy}>
                  공유 중지
                </button>
              </div>
            </>
          ) : (
            <>
              <div>
                <div className="card-title">아직 공유 전이에요</div>
                <div className="muted-2">시작하면 아래 주소로 참여자가 접속할 수 있어요.</div>
              </div>
              <code className="url pending-url">{preview}</code>
              <button className="big-btn" onClick={() => void start()} disabled={busy || !port || !hasSheet}>
                {busy ? "시작하는 중…" : "공유 시작"}
              </button>
            </>
          )}
          <div className="small muted">참여 방식: 이름만 입력 · 명단 외 이름은 승인 후 배정</div>
          {error && <div className="error small">{error}</div>}
        </section>
      </div>

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
