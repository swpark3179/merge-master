import type { AgentConnection, AgentServiceView, ExportOptions, HandoffCheck, SheetModel } from "@mm/shared";
import { formatDeadline, parseDeadline } from "@mm/shared";
import { useEffect, useState } from "react";

import { appApi, errorText } from "../../api";
import { useApi } from "../../job";
import { AgentFolderDialog, badName } from "./AgentFolderDialog";

/** 호스트 화면이 고르게 하는 가장 이른 마감(지금부터 분) — Rust `agent_protocol::HOST_LEAD_MINUTES`. */
const LEAD_MINUTES = 5;
const MARK = 0xfff3b0;
const RESERVED_SLUGS = ["api", "ws", "assets", "__dev", "agent", "index.html", "favicon.ico", "robots.txt"];

/** 에이전트 PC의 지금 현지 시각(UTC 필드로 읽는다 — 이 PC 시간대와 상관없이). */
function agentWallNow(conn: AgentConnection): Date {
  return new Date(Date.now() + conn.clockSkewMs + conn.info.utcOffsetMinutes * 60_000);
}

/** 에이전트 PC 시각 기준 마감(`YYYY-MM-DDTHH:MM`) → 유닉스 ms. */
function agentDeadlineMs(text: string, conn: AgentConnection): number | null {
  const d = parseDeadline(text);
  if (!d) return null;
  const wall = Date.UTC(d.getFullYear(), d.getMonth(), d.getDate(), d.getHours(), d.getMinutes());
  return wall - conn.info.utcOffsetMinutes * 60_000;
}

function pad(n: number) {
  return String(n).padStart(2, "0");
}

/** 에이전트 PC의 오늘(`YYYY-MM-DD`) — 마감 날짜 칸의 가장 이른 날. */
export function agentToday(conn: AgentConnection): string {
  const n = agentWallNow(conn);
  return `${n.getUTCFullYear()}-${pad(n.getUTCMonth() + 1)}-${pad(n.getUTCDate())}`;
}

/** '에이전트 PC 시각 기준 (지금 14:20)'. */
export function AgentClock({ conn }: { conn: AgentConnection | null }) {
  const [, setTick] = useState(0);
  useEffect(() => {
    const id = window.setInterval(() => setTick((n) => n + 1), 30_000);
    return () => window.clearInterval(id);
  }, []);
  if (!conn) return <span className="muted small">에이전트에 연결하면 그 PC 시각으로 잽니다</span>;
  const n = agentWallNow(conn);
  const skewMin = Math.round(conn.clockSkewMs / 60_000);
  return (
    <span className="muted small">
      에이전트 PC 시각 기준 (지금 {pad(n.getUTCHours())}:{pad(n.getUTCMinutes())})
      {Math.abs(skewMin) >= 2 && <span className="warn"> · 이 PC 시계와 {Math.abs(skewMin)}분 다릅니다</span>}
    </span>
  );
}

/** 에이전트에서 서비스할 마감이 안 되는 까닭(되면 `null`). */
export function deadlineProblem(deadline: string | undefined, conn: AgentConnection | null): string | null {
  if (!deadline) return "에이전트에서 서비스하려면 입력 마감(날짜·시·분)을 정해 주세요";
  if (!conn) return null;
  const due = agentDeadlineMs(deadline, conn);
  const now = Date.now() + conn.clockSkewMs;
  if (due === null) return "마감 일시를 날짜·시·분까지 정해 주세요";
  if (due < now + LEAD_MINUTES * 60_000)
    return `마감은 지금(에이전트 PC 시각)부터 ${LEAD_MINUTES}분 뒤보다 늦어야 해요`;
  if (due > now + conn.info.maxDays * 86_400_000) return `마감은 ${conn.info.maxDays}일 안으로 정해 주세요`;
  return null;
}

/** 원본 파일 이름 → (이름, 확장자). 새 문서(확장자 없음)는 xlsx. */
function splitWorkbook(name: string): [string, string] {
  const m = /^(.*)\.(xlsx|xlsm|xlsb|xls)$/i.exec(name);
  return m ? [m[1]!, m[2]!.toLowerCase()] : [name || "취합", "xlsx"];
}

/** `budget_취합_20261030.xlsx`(마감 날짜). */
function defaultFileName(workbook: string, deadline: string | undefined): string {
  const [stem, ext] = splitWorkbook(workbook);
  const d = parseDeadline(deadline);
  const day = d ? `${d.getFullYear()}${pad(d.getMonth() + 1)}${pad(d.getDate())}` : "";
  return `${stem}_취합${day ? `_${day}` : ""}.${ext}`;
}

function slugProblem(slug: string): string | null {
  if (!/^[A-Za-z0-9_-]{1,32}$/.test(slug)) return "주소 경로는 영문·숫자·-·_ 1~32자로 정해 주세요";
  if (RESERVED_SLUGS.includes(slug.toLowerCase())) return `‘${slug}’은(는) 주소 경로로 쓸 수 없어요`;
  return null;
}

interface Props {
  conn: AgentConnection | null;
  onConn: (c: AgentConnection | null) => void;
  /** 입력 마감(필수). */
  deadline: string | undefined;
  hasSheet: boolean;
  sheets: SheetModel[];
  /** 이 업무의 주소 경로(정해 둔 값). */
  slug: string;
  myName: string;
  /** 공유를 시작하기 전에(내 이름 저장 등). */
  beforeStart: () => Promise<void>;
  onHandedOff: (view: AgentServiceView) => void;
}

/**
 * 1c '에이전트 PC'에서 공유: 에이전트에 연결하고, 마감 때 저장할 곳(에이전트 기본 저장 폴더 아래)을 골라 넘긴다. 넘기면
 * 이 PC의 업무는 끝나고 사이드바 '에이전트에서 진행 중'에 보인다 — 이 PC를 꺼도 에이전트가 마감까지 서비스한다.
 */
export function AgentHandoff(props: Props) {
  const { conn, onConn, deadline, sheets } = props;
  const api = useApi();
  const [addr, setAddr] = useState("");
  const [connecting, setConnecting] = useState(false);
  const [dir, setDir] = useState<string[]>([]);
  const [picking, setPicking] = useState(false);
  const workbook = sheets[0]?.workbook ?? "";
  const [fileName, setFileName] = useState(() => defaultFileName(workbook, deadline));
  const [fileEdited, setFileEdited] = useState(false);
  const [slug, setSlug] = useState(props.slug || "q4");
  const [history, setHistory] = useState(false); // 저장 화면과 같게 '변경 이력' 시트는 기본으로 넣지 않는다
  const [mark, setMark] = useState(false);
  const [styles, setStyles] = useState(true);
  const [cancelled, setCancelled] = useState(true);
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [check, setCheck] = useState<HandoffCheck | null>(null);

  useEffect(() => {
    void appApi.agentLastAddr().then((a) => a && setAddr((cur) => cur || a));
  }, []);
  // 파일 이름을 고치지 않았으면 마감 날짜를 따라간다.
  useEffect(() => {
    if (!fileEdited) setFileName(defaultFileName(workbook, deadline));
  }, [workbook, deadline, fileEdited]);

  const connect = async () => {
    setConnecting(true);
    setError(null);
    try {
      const c = await appApi.agentConnect(addr);
      setAddr(c.addr.replace(/^http:\/\//, ""));
      onConn(c);
    } catch (e) {
      onConn(null);
      setError(errorText(e));
    } finally {
      setConnecting(false);
    }
  };

  const [, ext] = splitWorkbook(workbook);
  const fileProblem =
    badName(fileName) ??
    (fileName.toLowerCase().endsWith(`.${ext}`) ? null : `파일 이름은 원본과 같은 형식(.${ext})으로 끝나야 해요`);
  const sProblem = slugProblem(slug.trim());
  const dProblem = deadlineProblem(deadline, conn);
  const info = conn?.info;
  const sep = "\\";
  const target = info ? [info.baseDir, ...dir, fileName].join(sep) : "";
  const preview = info ? `${(info.urls[0] ?? "http://에이전트/").replace(/\/$/, "")}/${slug.trim()}` : "";
  const ready =
    !!conn && props.hasSheet && !!props.myName.trim() && !dProblem && !fileProblem && !sProblem && busy === null;
  const exportOptions: ExportOptions = { mark: mark ? MARK : undefined, styles, history, cancelled };

  const start = async () => {
    setError(null);
    setBusy("확인하는 중…");
    try {
      await props.beforeStart();
      setCheck(await api.agentHandoffCheck());
    } catch (e) {
      setError(errorText(e));
    } finally {
      setBusy(null);
    }
  };

  const send = async (force: boolean) => {
    if (!conn || !deadline) return;
    setBusy("넘기는 중…");
    setError(null);
    try {
      const view = await api.agentHandoff({
        addr: conn.addr,
        deadline,
        dir,
        fileName: fileName.trim(),
        export: exportOptions,
        slug: slug.trim(),
        force,
      });
      setCheck(null);
      props.onHandedOff(view);
    } catch (e) {
      setError(errorText(e));
    } finally {
      setBusy(null);
    }
  };

  return (
    <section className="card share agent-share" aria-label="에이전트">
      <div className="card-title">에이전트 PC에서 공유</div>
      <div className="field">
        <span>에이전트 주소</span>
        <div className="row">
          <input
            aria-label="에이전트 주소"
            className="grow"
            placeholder="PC 이름 또는 IP (예: 10.0.0.7)"
            value={addr}
            onChange={(e) => setAddr(e.target.value)}
            onKeyDown={(e) => e.key === "Enter" && addr.trim() && void connect()}
          />
          <button className="ghost" onClick={() => void connect()} disabled={!addr.trim() || connecting}>
            {connecting ? "확인 중…" : "연결 확인"}
          </button>
        </div>
        {info && (
          <div className="small agent-info" aria-label="에이전트 정보">
            <span className="ok">● {info.name}</span> <span className="muted">· v{info.version}</span>
            {info.version !== conn.appVersion && <span className="muted"> (이 앱 v{conn.appVersion})</span>}
            {!info.excel && (
              <div className="warn small">
                이 에이전트 PC에는 Excel이 없어요. 마감 때 취합 데이터를 남겨 두고, 이 앱이 접속하면 이 PC에서 엑셀로
                저장해요.
              </div>
            )}
            {!info.baseOk && (
              <div className="warn small">
                기본 저장 폴더({info.baseDir})를 쓸 수 없어요. 에이전트 PC의 트레이 메뉴에서 바꿔 주세요.
              </div>
            )}
          </div>
        )}
      </div>

      {info && (
        <>
          <div className="field">
            <span>
              <b>마감 때 저장할 곳</b> <span className="muted small">(에이전트 PC)</span>
            </span>
            <div className="row">
              <code className="url grow" title={target}>
                {[info.baseDir, ...dir].join(sep)}
              </code>
              <button className="ghost" onClick={() => setPicking(true)}>
                폴더 고르기…
              </button>
            </div>
          </div>
          <label className="field">
            <span>파일 이름</span>
            <input
              aria-label="저장 파일 이름"
              className={fileProblem ? "invalid" : undefined}
              value={fileName}
              onChange={(e) => {
                setFileName(e.target.value);
                setFileEdited(true);
              }}
            />
            {fileProblem && <span className="warn small">{fileProblem}</span>}
            <span className="muted small">같은 이름의 파일이 있으면 _2, _3을 붙여 저장해요(덮어쓰지 않음).</span>
          </label>
          <label className="field">
            <span>주소 경로</span>
            <input
              aria-label="에이전트 주소 경로"
              className={sProblem ? "invalid" : undefined}
              value={slug}
              onChange={(e) => setSlug(e.target.value.replace(/[^\w-]/g, ""))}
            />
            {sProblem ? (
              <span className="warn small">{sProblem}</span>
            ) : (
              <span className="muted small">다른 업무가 쓰고 있으면 에이전트가 {slug.trim()}-2처럼 바꿔요.</span>
            )}
          </label>
          <details className="more">
            <summary>함께 넣기</summary>
            <div className="more-body save-options">
              <label className="check">
                <input type="checkbox" checked={history} onChange={(e) => setHistory(e.target.checked)} />
                ‘변경 이력’ 시트 추가
              </label>
              <label className="check">
                <input type="checkbox" checked={mark} onChange={(e) => setMark(e.target.checked)} />
                바뀐 칸을 노란색으로 표시
              </label>
              <label className="check">
                <input type="checkbox" checked={styles} onChange={(e) => setStyles(e.target.checked)} />
                앱에서 바꾼 서식·병합·크기도 반영
              </label>
              <label className="check">
                <input type="checkbox" checked={cancelled} onChange={(e) => setCancelled(e.target.checked)} />
                취소 표시한 행: 취소선 + 회색 채우기
              </label>
            </div>
          </details>
          <code className="url pending-url">{preview}</code>
        </>
      )}

      <button className="big-btn" onClick={() => void start()} disabled={!ready}>
        {busy ?? "에이전트에서 공유 시작"}
      </button>
      {props.hasSheet && !props.myName.trim() && (
        <div className="warn small">접속 조건에 내 이름을 입력하면 넘길 수 있어요.</div>
      )}
      {dProblem && <div className="warn small">{dProblem}</div>}
      <div className="small muted">
        넘기면 이 PC의 업무는 끝나고(기록은 남아요), 이 PC를 꺼도 에이전트가 마감까지 서비스해요. 마감 시각이 되면 위
        위치에 엑셀로 저장하고 서비스를 끝냅니다.
      </div>
      {error && <div className="error small">{error}</div>}

      {picking && info && conn && (
        <AgentFolderDialog
          addr={conn.addr}
          baseDir={info.baseDir}
          initial={dir}
          onClose={() => setPicking(false)}
          onPick={(d) => {
            setDir(d);
            setPicking(false);
          }}
        />
      )}
      {check && conn && deadline && (
        <HandoffConfirm
          check={check}
          agentName={conn.info.name}
          deadline={deadline}
          target={target}
          busy={busy}
          onCancel={() => setCheck(null)}
          onConfirm={(force) => void send(force)}
        />
      )}
    </section>
  );
}

function HandoffConfirm(props: {
  check: HandoffCheck;
  agentName: string;
  deadline: string;
  target: string;
  busy: string | null;
  onCancel: () => void;
  onConfirm: (force: boolean) => void;
}) {
  const { check, busy } = props;
  const [force, setForce] = useState(false);
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && !busy && props.onCancel();
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [busy, props]);
  return (
    <div className="modal-back" onClick={() => !busy && props.onCancel()}>
      <div
        className="modal card handoff-confirm"
        role="alertdialog"
        aria-label="에이전트로 넘기기"
        onClick={(e) => e.stopPropagation()}
      >
        <b>‘{props.agentName}’에서 공유를 시작할까요?</b>
        <ul className="small">
          <li>
            마감: <b>{formatDeadline(props.deadline)}</b> (에이전트 PC 시각) — 이때 엑셀로 저장하고 서비스를 끝냅니다.
          </li>
          <li>
            저장할 곳: <span className="mono">{props.target}</span>
          </li>
          <li>
            넘기면 이 PC의 업무는 끝나요. 진행 상황은 사이드바 ‘에이전트에서 진행 중’에서 봐요(승인·지금 저장도 거기서).
          </li>
          {check.conflicts > 0 && (
            <li className="warn">
              불러온 뒤 Excel에서도 고친 칸이 {check.conflicts.toLocaleString()}개 있어요. 마감 때 그 칸에는 취합한 값을
              씁니다.
            </li>
          )}
          {check.suspicious && (
            <li className="warn">
              원본의 표 위치가 바뀐 것 같아요(행·열을 넣거나 지움). 이대로 넘기면 마감 때 엉뚱한 칸에 쓸 수 있어요.
              <label className="check">
                <input type="checkbox" checked={force} onChange={(e) => setForce(e.target.checked)} />
                확인했어요, 그래도 넘기기
              </label>
            </li>
          )}
        </ul>
        <div className="row">
          <span className="spacer" />
          <button className="ghost" onClick={props.onCancel} disabled={!!busy}>
            취소
          </button>
          <button onClick={() => props.onConfirm(force)} disabled={!!busy || (check.suspicious && !force)} autoFocus>
            {busy ?? "넘기기"}
          </button>
        </div>
      </div>
    </div>
  );
}
