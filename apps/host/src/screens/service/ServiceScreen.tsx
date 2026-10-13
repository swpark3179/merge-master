import type {
  CollectConfig,
  DeadlineAction,
  HostSnapshot,
  IpNamesView,
  JobSummary,
  LanAddress,
  RecalcStatus,
  ServerStatus,
  ServiceInfo,
  SheetModel,
} from "@mm/shared";
import { formatDeadline } from "@mm/shared";
import { listen } from "@tauri-apps/api/event";
import { useCallback, useEffect, useRef, useState } from "react";

import { errorText, jobEvent } from "../../api";
import { useApi, useJobId } from "../../job";
import { useToast } from "../../frame";
import { IpNamesPanel } from "./IpNamesPanel";
import { ParticipantsPanel } from "./ParticipantsPanel";

interface Props {
  active: boolean;
  hasSheet: boolean;
  service: ServiceInfo | null;
  onService: (s: ServiceInfo | null) => void;
  /** 이 업무의 주소 경로(정해 둔 값). */
  slug: string;
  /** 모든 업무(같은 포트를 쓰는 다른 업무·같은 업무명 확인). */
  jobs: JobSummary[];
  snapshot: HostSnapshot | null;
  /** 확정한 시트들(현재 위치 주소·승인 범위의 시트). */
  sheets: SheetModel[];
  onGoConfig: () => void;
}

/**
 * 1c 서비스 시작 · 접속자 관리. 참여자 서버는 하나(포트는 모든 업무 공통)이고, 이 업무를 주소 경로로 공유하거나 멈춘다
 * (다른 업무의 공유는 그대로). 참여자는 업무 목록 주소(`http://IP:포트/`)에서 업무를 골라 들어올 수도 있다.
 */
export function ServiceScreen(props: Props) {
  const { active, hasSheet, service, onService, snapshot, sheets, onGoConfig, jobs } = props;
  const api = useApi();
  const job = useJobId();
  const [config, setConfig] = useState<CollectConfig | null>(null);
  const [roster, setRoster] = useState<string[]>([]);
  const [lans, setLans] = useState<LanAddress[]>([]);
  const [ip, setIp] = useState<string | null>(null);
  // 단계를 오가면 이 화면은 새로 그려진다: 공유 중이면 실제 포트·경로를 보인다.
  const [port, setPort] = useState(() => String(service?.port ?? 8080));
  const [slug, setSlug] = useState(() => service?.slug ?? (props.slug || "q4"));
  /** 참여자 서버(모든 업무 공통). 다른 업무가 공유 중이면 포트를 바꿀 수 없다. */
  const [server, setServer] = useState<ServerStatus | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [net, setNet] = useState(false);
  /** 이 화면에서 공유를 시작한 시각(표시용). */
  const [since, setSince] = useState<string | null>(null);
  const toast = useToast();
  const [recalc, setRecalc] = useState<RecalcStatus | null>(null);
  /** IP 명단이 바뀌었다(승인하며 'IP 명단에 저장'). */
  const [ipRev, setIpRev] = useState(0);
  /** 내 이름(설정의 요청자): 공지·강조·채팅에 이 이름으로 보인다. 공유를 시작하려면 있어야 한다. */
  const [myName, setMyName] = useState("");
  const savedName = config?.requester;
  useEffect(() => {
    if (savedName !== undefined) setMyName(savedName);
  }, [savedName]);
  /** IP 명단에 있는 이 PC(호스트)의 이름. 없으면 `null`. */
  const [pcName, setPcName] = useState<string | null>(null);
  const onIpNames = useCallback(
    (v: IpNamesView) => setPcName(v.entries.find((e) => v.hostIps.includes(e.ip))?.name ?? null),
    [],
  );

  // 수식 재계산(Excel 계산 사본) 상태
  useEffect(() => {
    void api.recalcStatus().then(setRecalc);
    const off = listen<RecalcStatus>(jobEvent("recalc-status", job), (e) => setRecalc(e.payload));
    return () => void off.then((f) => f());
  }, []);

  const reload = useCallback(async () => {
    if (!hasSheet) return;
    try {
      // 명단은 모든 시트의 합집합이고, 제목·안내·마감은 모든 시트가 같다: 첫 시트로 읽는다.
      const v = await api.configView(0, null);
      setConfig(v.config);
      setRoster(v.roster);
    } catch (e) {
      setError(errorText(e));
    }
  }, [hasSheet]);

  useEffect(() => {
    if (!active) return;
    void reload();
    void api.serverStatus().then((st) => {
      setServer(st);
      if (!service) setPort(String(st.port));
    });
    void api.lanAddresses().then((l) => {
      setLans(l);
      setIp((cur) => cur ?? l[0]?.ip ?? "127.0.0.1");
    });
  }, [active, reload]);

  const url = service ? `http://${ip ?? service.urls[0]?.ip ?? "127.0.0.1"}:${service.port}/${service.slug}` : null;
  const listUrl = service ? `http://${ip ?? service.urls[0]?.ip ?? "127.0.0.1"}:${service.port}/` : null;
  /** 다른 업무가 공유 중이다(포트는 그 업무들과 같이 쓴다). */
  const others = jobs.filter((j) => j.id !== job && j.published);
  const portLocked = !!service || ((server?.running ?? false) && others.length > 0);
  /** 같은 업무명으로 공유 중인 다른 업무(참여자 목록에서 헷갈린다). */
  const sameTitle = config?.title ? others.filter((j) => j.title === config.title) : [];
  const slugTaken = jobs.some((j) => j.id !== job && j.slug.toLowerCase() === slug.trim().toLowerCase());

  const saveMeta = async (patch: Partial<Pick<CollectConfig, "title" | "description" | "requester" | "deadline">>) => {
    if (!config) return;
    const next = { ...config, ...patch };
    try {
      const v = await api.configApply(
        0,
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

  const setDeadlineAction = async (deadlineAction: DeadlineAction) => {
    if (!config) return;
    // 마감 동작은 모든 시트에 같이 걸린다(다른 권한은 첫 시트 것 그대로).
    try {
      const v = await api.configApply(
        0,
        { op: "setPermissions", permissions: { ...config.permissions, deadlineAction } },
        null,
      );
      setConfig(v.config);
    } catch (e) {
      setError(errorText(e));
    }
  };

  // 내 이름이 비어 있고 이 PC의 IP가 IP 명단에 있으면 그 이름으로 채워 저장한다(입력 중인 글은 덮지 않는다, 한 번만).
  const autoFilled = useRef<string | null>(null);
  useEffect(() => {
    if (!config || config.requester.trim() || !pcName || myName.trim() || autoFilled.current === pcName) return;
    autoFilled.current = pcName;
    setMyName(pcName);
    void saveMeta({ requester: pcName });
    // saveMeta·myName은 그때의 것을 쓴다(설정·명단이 바뀔 때만 다시 본다).
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [config, pcName]);

  /** 내 이름을 저장한다. 비우면 저장하지 않고 앞의 이름으로 돌린다(필수). */
  const saveName = async () => {
    if (!config) return;
    const name = myName.trim();
    if (!name) return setMyName(config.requester);
    if (name !== config.requester) await saveMeta({ requester: name });
  };

  const start = async () => {
    setBusy(true);
    setError(null);
    try {
      await saveName();
      onService(await api.servicePublish(Number(port), slug.trim() || "q4"));
      setSince(new Date().toTimeString().slice(0, 5));
      toast("이 업무의 공유를 시작했어요");
    } catch (e) {
      setError(errorText(e));
    } finally {
      setBusy(false);
    }
  };

  const stop = async () => {
    setBusy(true);
    try {
      await api.serviceUnpublish();
      onService(null);
      setSince(null);
      toast(others.length > 0 ? "이 업무의 공유를 멈췄어요(다른 업무는 계속)" : "공유를 멈췄어요");
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
      others.length > 0 ? `(다른 취합 업무 목록: ${listUrl})` : "",
      "같은 사내망에서 위 주소를 여세요. 등록된 PC는 바로 들어오고, 그 밖은 이름을 입력하면 호스트가 승인합니다.",
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
                  <b>업무명 (취합 제목)</b>
                  <input
                    aria-label="취합 제목"
                    defaultValue={config.title}
                    key={`t${config.title}`}
                    onBlur={(e) => e.target.value !== config.title && void saveMeta({ title: e.target.value })}
                  />
                </label>
                <label className="field">
                  <span>
                    <b>내 이름</b> <span className="muted">(필수)</span>
                  </span>
                  <input
                    aria-label="내 이름"
                    aria-required="true"
                    aria-invalid={!myName.trim()}
                    className={myName.trim() ? undefined : "invalid"}
                    placeholder="예: 박지훈 (기획팀)"
                    title="공지·셀 강조·채팅에 이 이름으로 보입니다. 이 PC의 IP가 IP 명단에 있으면 그 이름으로 채워집니다."
                    value={myName}
                    onChange={(e) => setMyName(e.target.value)}
                    onBlur={() => void saveName()}
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
                  <HostClock />
                </label>
                <fieldset className="deadline-action" disabled={!config.deadline}>
                  <legend>마감 시각이 되면</legend>
                  {(
                    [
                      ["readOnly", "읽기 전용으로 전환", "참여자는 접속한 채로 보기만 합니다"],
                      ["stopSharing", "공유 닫기", "이 업무의 공유를 닫아 참여자 연결이 끊깁니다(다시 열 수 있음)"],
                    ] as const
                  ).map(([value, label, hint]) => (
                    <label key={value} className="radio" title={hint}>
                      <input
                        type="radio"
                        name="deadline-action"
                        checked={config.permissions.deadlineAction === value}
                        onChange={() => void setDeadlineAction(value)}
                      />
                      {label}
                    </label>
                  ))}
                </fieldset>
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
                <span>
                  포트 <span className="muted">(모든 업무 공통)</span>
                </span>
                <input
                  aria-label="포트"
                  value={port}
                  onChange={(e) => setPort(e.target.value.replace(/\D/g, ""))}
                  disabled={portLocked}
                  title={
                    !service && portLocked ? `다른 업무가 포트 ${server?.port}로 공유 중이라 바꿀 수 없어요` : undefined
                  }
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
                {slugTaken && <span className="warn small">다른 업무가 쓰는 주소 경로예요</span>}
              </label>
            </div>
          )}
          {others.length > 0 && (
            <div className="small muted">
              같은 포트({server?.port})로 공유 중인 다른 업무: {others.map((j) => j.title || "새 취합").join(", ")}
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
              {listUrl && (
                <div className="url-row list-url">
                  <span className="muted small">업무 목록</span>
                  <code className="url">{listUrl}</code>
                  <button className="ghost" onClick={() => void copy(listUrl, "업무 목록 주소를 복사했어요")}>
                    복사
                  </button>
                </div>
              )}
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
              <button
                className="big-btn"
                onClick={() => void start()}
                disabled={busy || !port || !hasSheet || !myName.trim() || slugTaken}
              >
                {busy ? "시작하는 중…" : "공유 시작"}
              </button>
              {hasSheet && config && !myName.trim() && (
                <div className="warn small">접속 조건에 내 이름을 입력하면 공유를 시작할 수 있어요.</div>
              )}
            </>
          )}
          {sameTitle.length > 0 && (
            <div className="warn small">
              같은 업무명으로 공유 중인 업무가 있어요. 참여자가 업무 목록에서 헷갈리지 않게 업무명을 다르게 해 주세요.
            </div>
          )}
          <div className="small muted">
            참여 방식: IP 명단의 PC는 그 이름으로 바로 · 그 밖은 이름 입력 후 호스트 승인
          </div>
          {error && <div className="error small">{error}</div>}
        </section>
      </div>

      <ParticipantsPanel
        running={service !== null}
        snapshot={snapshot}
        roster={roster}
        sheets={sheets}
        onRosterChanged={reload}
        onIpNamesChanged={() => setIpRev((n) => n + 1)}
        onGoConfig={onGoConfig}
        onError={setError}
      />

      <IpNamesPanel snapshot={snapshot} rev={ipRev} onError={setError} onChange={onIpNames} />
    </div>
  );
}

/** 마감 시각은 이 PC(호스트) 시계로 잰다: 지금 시각을 함께 보인다(30초마다). */
function HostClock() {
  const [now, setNow] = useState(() => new Date());
  useEffect(() => {
    const id = window.setInterval(() => setNow(new Date()), 30_000);
    return () => window.clearInterval(id);
  }, []);
  const hm = [now.getHours(), now.getMinutes()].map((n) => String(n).padStart(2, "0")).join(":");
  return <span className="muted small">호스트 PC 시각 기준 (지금 {hm})</span>;
}
