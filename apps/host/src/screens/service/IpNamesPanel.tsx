import type { HostSnapshot, IpNamesView } from "@mm/shared";
import { open as openDialog, save as saveDialog } from "@tauri-apps/plugin-dialog";
import { useCallback, useEffect, useRef, useState } from "react";

import { errorText } from "../../api";
import { useApi } from "../../job";
import { useToast } from "../../frame";

interface Props {
  snapshot: HostSnapshot | null;
  /** 바뀔 때마다 새로 읽는다(승인하며 'IP 명단에 저장'). */
  rev: number;
  onError: (e: string) => void;
  /** 명단을 읽거나 바꿀 때마다(1c '내 이름'을 이 PC의 이름으로 채운다). */
  onChange?: (view: IpNamesView) => void;
  /** 에이전트로 넘긴 업무면 그 에이전트 PC 이름(이 명단도 보내지만 에이전트 PC 명단이 먼저다). */
  agentName?: string;
}

const JSON_FILTER = [{ name: "JSON", extensions: ["json"] }];

/**
 * 1c IP 명단: IP별 참여자 이름. 명단의 IP에서 접속하면 이름을 묻지 않고 그 이름으로 바로 들어오고, 명단 밖 IP는 이름을
 * 입력한 뒤 호스트가 승인한다. 이 PC(호스트)의 IP가 명단에 있으면 그 이름이 호스트 이름이다. JSON 파일로 내보내고 불러온다.
 * 처음엔 머리 줄(이름·명수·JSON 단추)만 보이게 접어 둔다. 접어도 명단은 읽는다('내 이름'을 채운다).
 */
export function IpNamesPanel({ snapshot, rev, onError, onChange, agentName }: Props) {
  const api = useApi();
  const [view, setViewState] = useState<IpNamesView | null>(null);
  const [ip, setIp] = useState("");
  const [name, setName] = useState("");
  const [open, setOpen] = useState(false);
  const toast = useToast();
  const changed = useRef(onChange);
  changed.current = onChange;
  const setView = useCallback((v: IpNamesView) => {
    setViewState(v);
    changed.current?.(v);
  }, []);

  const reload = useCallback(() => {
    api
      .ipNamesGet()
      .then(setView)
      .catch((e) => onError(errorText(e)));
  }, [onError, setView]);

  useEffect(() => reload(), [reload, rev]);

  const run = async (f: () => Promise<IpNamesView>, done?: string) => {
    try {
      setView(await f());
      if (done) toast(done);
      return true;
    } catch (e) {
      onError(errorText(e));
      return false;
    }
  };

  const add = async () => {
    if (!ip.trim() || !name.trim()) return;
    if (await run(() => api.ipNamesPut(ip.trim(), name.trim()))) {
      setIp("");
      setName("");
    }
  };

  const remove = (target: string) =>
    view && void run(() => api.ipNamesSet(view.entries.filter((e) => e.ip !== target)));

  const rename = (target: string, next: string) => {
    const old = view?.entries.find((e) => e.ip === target)?.name;
    if (next.trim() && next.trim() !== old) void run(() => api.ipNamesPut(target, next.trim()));
  };

  const importFile = async () => {
    const path = await openDialog({ multiple: false, filters: JSON_FILTER });
    if (typeof path !== "string") return;
    if (await run(() => api.ipNamesImport(path), "IP 명단을 불러왔어요")) setOpen(true); // 불러온 명단을 바로 보인다
  };

  const exportFile = async () => {
    const path = await saveDialog({ defaultPath: "IP명단.json", filters: JSON_FILTER });
    if (!path) return;
    try {
      const n = await api.ipNamesExport(path);
      toast(`IP 명단 ${n}명을 내보냈어요`);
    } catch (e) {
      onError(errorText(e));
    }
  };

  const entries = view?.entries ?? [];
  const hostIps = view?.hostIps ?? [];
  const hostEntry = entries.find((e) => hostIps.includes(e.ip));
  // 지금 접속 중인 IP(명단 이름 → 상태).
  const online = new Map(
    (snapshot?.participants ?? []).filter((p) => p.status !== "offline" && p.ip).map((p) => [p.ip!, p.name]),
  );

  return (
    <section className="ipnames" aria-label="IP 명단">
      <div className="card-head">
        <button
          className="net-toggle ip-toggle"
          aria-expanded={open}
          aria-controls={open ? "ipnames-body" : undefined}
          onClick={() => setOpen((v) => !v)}
        >
          <svg
            className="chev"
            viewBox="0 0 16 16"
            fill="none"
            stroke="currentColor"
            strokeWidth="2"
            strokeLinecap="round"
            strokeLinejoin="round"
            aria-hidden
          >
            <path d="M6 3.5L10.5 8 6 12.5" />
          </svg>
          <b className="big">IP 명단</b>
          <span className="tag" title="이 PC의 모든 취합 업무가 같은 명단을 씁니다">
            모든 업무 공통
          </span>
          <span className="muted small">{entries.length}명</span>
          <span className="muted small">{open ? "접기" : "펼치기"}</span>
        </button>
        <span className="spacer" />
        <button className="ghost small" onClick={() => void importFile()}>
          JSON 불러오기
        </button>
        <button className="ghost small" onClick={() => void exportFile()} disabled={entries.length === 0}>
          JSON 내보내기
        </button>
      </div>
      {open && (
        <div id="ipnames-body" className="ipnames-body">
          <div className="muted small">
            명단의 IP에서 열면 어느 업무든 이름을 묻지 않고 그 이름으로 바로 들어옵니다 · 명단 밖 IP는 이름을 입력하고
            승인을 기다립니다
          </div>
          <div className="host-line small">
            <span className="muted">이 PC(호스트)</span> <span className="mono">{hostIps.join(", ") || "–"}</span> →{" "}
            {hostEntry ? (
              <b>{hostEntry.name}</b>
            ) : (
              <>
                <b>{snapshot?.hostName ?? "호스트"}</b>
                <span className="muted"> (명단에 이 PC의 IP를 넣으면 그 이름으로 보입니다)</span>
                {hostIps[0] && (
                  <button className="link" onClick={() => setIp(hostIps[0]!)}>
                    이 PC IP 넣기
                  </button>
                )}
              </>
            )}
          </div>
          <div className="iptable" role="table" aria-label="IP별 이름">
            <div className="ipr iphead" role="row">
              <span role="columnheader">IP</span>
              <span role="columnheader">이름</span>
              <span role="columnheader">상태</span>
              <span />
            </div>
            {entries.length === 0 && (
              <div className="muted small ipempty">
                아직 없습니다. 아래에서 IP와 이름을 넣거나 JSON 파일을 불러오세요.
              </div>
            )}
            {entries.map((e) => (
              <div key={e.ip} className="ipr" role="row" aria-label={e.ip}>
                <span className="mono">{e.ip}</span>
                <input
                  aria-label={`${e.ip} 이름`}
                  defaultValue={e.name}
                  key={e.name}
                  maxLength={20}
                  onBlur={(ev) => rename(e.ip, ev.target.value)}
                  onKeyDown={(ev) => ev.key === "Enter" && !ev.nativeEvent.isComposing && ev.currentTarget.blur()}
                />
                <span className="small">
                  {hostIps.includes(e.ip) ? (
                    <span className="muted">호스트</span>
                  ) : online.get(e.ip) ? (
                    <span className="ok">● 접속 중</span>
                  ) : (
                    <span className="muted">–</span>
                  )}
                </span>
                <button className="x" aria-label={`${e.ip} 빼기`} onClick={() => remove(e.ip)}>
                  ✕
                </button>
              </div>
            ))}
            <form
              className="ipr ipadd"
              onSubmit={(ev) => {
                ev.preventDefault();
                void add();
              }}
            >
              <input
                aria-label="추가할 IP"
                className="mono"
                placeholder="IP (예: 192.168.0.11)"
                value={ip}
                onChange={(ev) => setIp(ev.target.value.replace(/[^\d.:a-fA-F]/g, ""))}
              />
              <input
                aria-label="추가할 이름"
                placeholder="이름"
                maxLength={20}
                value={name}
                onChange={(ev) => setName(ev.target.value)}
              />
              <span />
              <button type="submit" className="small" disabled={!ip.trim() || !name.trim()}>
                추가
              </button>
            </form>
          </div>
          {agentName && (
            <div className="muted small">
              이 명단은 에이전트 PC({agentName})에도 보냅니다. 에이전트 PC의 명단(트레이 ‘IP 명단 관리’)에 같은 IP가
              있으면 그쪽 이름이 먼저예요.
            </div>
          )}
          {view?.path && <div className="muted small">명단 파일: {view.path}</div>}
        </div>
      )}
    </section>
  );
}
