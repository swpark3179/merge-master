import type { HostSnapshot, ParticipantRow, SheetCell, SheetModel } from "@mm/shared";
import { a1, personColor, sheetPrefix } from "@mm/shared";
import { useEffect, useState } from "react";

import { errorText } from "../../api";
import { useApi } from "../../job";

interface Props {
  running: boolean;
  snapshot: HostSnapshot | null;
  /** 서비스가 꺼져 있을 때 보일 명단. */
  roster: string[];
  /** 확정한 시트들(현재 위치 주소, 승인 범위의 시트). */
  sheets: SheetModel[];
  onRosterChanged: () => void;
  /** 승인하며 IP 명단에 저장했다. */
  onIpNamesChanged: () => void;
  onGoConfig: () => void;
  onError: (e: string) => void;
}

function StatusText({ p }: { p: ParticipantRow }) {
  if (p.status === "online") return <span className="ok">● 접속 중</span>;
  if (p.status === "away") return <span className="warn">◐ 자리 비움 {p.idleMins}분</span>;
  return <span className="muted">○ 미접속</span>;
}

/** 시트 주소(시트가 둘 이상이면 `인력계획!F7`). */
function where(sheets: SheetModel[], at: SheetCell): string {
  const m = sheets[at.sheet];
  const origin = m?.origin ?? { row: 0, col: 0 };
  const cell = a1(origin.row + at.row, origin.col + at.col);
  return sheets.length > 1 && m ? `${sheetPrefix(m.sheet)}${cell}` : cell;
}

/** 1c 접속자: 승인 대기 + 명단 표(상태·배정·현재 위치·필수 입력). */
export function ParticipantsPanel({
  running,
  snapshot,
  roster,
  sheets,
  onRosterChanged,
  onIpNamesChanged,
  onGoConfig,
  onError,
}: Props) {
  const api = useApi();
  const [approving, setApproving] = useState<string | null>(null);
  const [range, setRange] = useState("");
  /** 배정 범위의 시트(범위에 `시트!`를 적으면 그쪽이 먼저). */
  const [rangeSheet, setRangeSheet] = useState(0);
  const [note, setNote] = useState("");
  /** 승인하면서 그 IP를 IP 명단에 그 이름으로 저장(다음부터 바로 들어온다). */
  const [remember, setRemember] = useState(false);
  const [menu, setMenu] = useState<string | null>(null);
  const [newName, setNewName] = useState("");
  // ⋯ 메뉴는 Esc나 메뉴 밖을 누르면 닫는다(마우스가 벗어날 때만 닫히면 키보드로는 닫을 수 없다).
  const menuOpen = menu !== null;
  useEffect(() => {
    if (!menuOpen) return;
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && setMenu(null);
    const onDown = (e: MouseEvent) => !(e.target as Element).closest(".pmenu") && setMenu(null);
    window.addEventListener("keydown", onKey);
    window.addEventListener("mousedown", onDown);
    return () => {
      window.removeEventListener("keydown", onKey);
      window.removeEventListener("mousedown", onDown);
    };
  }, [menuOpen]);
  const rows: ParticipantRow[] =
    snapshot?.participants ??
    roster.map((name, i) => ({
      name,
      color: i,
      status: "offline",
      idleMins: 0,
      assignment: "",
      required: 0,
      filled: 0,
    }));
  const pending = snapshot?.pending ?? [];
  const online = snapshot?.online ?? 0;

  const run = async (f: () => Promise<unknown>) => {
    try {
      await f();
    } catch (e) {
      onError(errorText(e));
    }
  };

  return (
    <section className="participants" aria-label="접속자">
      <div className="card-head">
        <b className="big">접속자</b>
        <span className="muted">
          명단 {rows.length}명 · 접속 중 {online}
          {pending.length > 0 ? ` · 승인 대기 ${pending.length}` : ""}
        </span>
        <span className="spacer" />
        <form
          className="add-name"
          onSubmit={(e) => {
            e.preventDefault();
            if (!newName.trim()) return;
            void run(async () => {
              await api.configApply(0, { op: "addPerson", name: newName }, null);
              setNewName("");
              onRosterChanged();
            });
          }}
        >
          <input
            aria-label="이름 추가"
            placeholder="+ 이름 추가"
            value={newName}
            onChange={(e) => setNewName(e.target.value)}
          />
        </form>
      </div>
      {!running && (
        <div className="muted small">
          서비스를 시작하면 접속 상황이 실시간으로 보입니다. 멈춰도 입력한 값과 진행률은 그대로 남습니다.
        </div>
      )}

      {pending.map((p) => (
        <div key={p.name} className="pending" role="alert">
          <b>승인 대기</b>
          <span>
            ‘{p.name}’ — {rows.some((r) => r.name === p.name) ? "명단에 있는 이름" : "명단에 없는 이름"} · IP 명단 밖{" "}
            <span className="mono">{p.ip}</span>
          </span>
          <span className="spacer" />
          {approving === p.name ? (
            <form
              className="approve-form"
              onSubmit={(e) => {
                e.preventDefault();
                void run(async () => {
                  await api.participantApprove(p.name, rangeSheet, range.trim() || null, note);
                  if (remember && p.ip) {
                    await api.ipNamesPut(p.ip, p.name);
                    onIpNamesChanged();
                  }
                  setApproving(null);
                  setRange("");
                  setNote("");
                  onRosterChanged();
                });
              }}
            >
              {sheets.length > 1 && (
                <select
                  aria-label="배정 시트"
                  value={rangeSheet}
                  onChange={(e) => setRangeSheet(Number(e.target.value))}
                >
                  {sheets.map((m, i) => (
                    <option key={i} value={i}>
                      {m.sheet}
                    </option>
                  ))}
                </select>
              )}
              <input
                aria-label="배정 범위"
                className="mono"
                placeholder="범위 (예: H500:H620)"
                value={range}
                onChange={(e) => setRange(e.target.value.toUpperCase())}
              />
              <input aria-label="배정 메모" placeholder="메모" value={note} onChange={(e) => setNote(e.target.value)} />
              <label className="check small" title="다음부터 이 PC에서는 이름을 묻지 않고 바로 들어옵니다">
                <input type="checkbox" checked={remember} onChange={(e) => setRemember(e.target.checked)} />
                IP 명단에 저장
              </label>
              <button type="submit" className="small">
                승인
              </button>
              <button type="button" className="ghost small" onClick={() => setApproving(null)}>
                취소
              </button>
            </form>
          ) : (
            <>
              <button className="small" onClick={() => setApproving(p.name)}>
                배정 후 승인
              </button>
              <button className="ghost small" onClick={() => void run(() => api.participantReject(p.name))}>
                거부
              </button>
            </>
          )}
        </div>
      ))}

      <div className="ptable" role="table">
        <div className="ptr phead" role="row">
          <span role="columnheader">이름</span>
          <span role="columnheader">상태</span>
          <span role="columnheader">배정</span>
          <span role="columnheader">현재 위치</span>
          <span role="columnheader">필수 입력</span>
          <span />
        </div>
        {rows.map((p) => {
          const pct = p.required > 0 ? Math.round((p.filled / p.required) * 100) : 0;
          return (
            <div key={p.name} className={`ptr${p.status === "offline" ? " off" : ""}`} role="row" aria-label={p.name}>
              <span className="pname">
                <span
                  className="mmg-dot"
                  style={{
                    background: p.status === "offline" ? "transparent" : personColor(p.color),
                    border: `1.5px solid ${personColor(p.color)}`,
                  }}
                />
                {p.name}
              </span>
              <span title={p.ip ? `IP ${p.ip}` : undefined}>
                <StatusText p={p} />
              </span>
              <span className="small">{p.assignment}</span>
              <span className="mono small">
                {p.editing ? (
                  <span className="editing" title="편집 중(다른 사람에게 잠김)">
                    ✎ {where(sheets, p.editing)}
                  </span>
                ) : p.cursor ? (
                  where(sheets, p.cursor)
                ) : (
                  "–"
                )}
              </span>
              <span className="pprogress">
                <span className="bar">
                  <span style={{ width: `${pct}%` }} />
                </span>
                {p.filled.toLocaleString()} / {p.required.toLocaleString()}
              </span>
              <span className="pmenu">
                <button
                  className="ghost small"
                  aria-label={`${p.name} 메뉴`}
                  onClick={() => setMenu(menu === p.name ? null : p.name)}
                >
                  ⋯
                </button>
                {menu === p.name && (
                  <div className="menu" role="menu" onMouseLeave={() => setMenu(null)}>
                    <button
                      role="menuitem"
                      onClick={() => {
                        setMenu(null);
                        onGoConfig();
                      }}
                    >
                      배정 변경…
                    </button>
                    <button
                      role="menuitem"
                      disabled={!p.editing}
                      title={
                        p.editing
                          ? "편집기를 열어 둔 채 자리를 비웠을 때 다른 사람이 입력할 수 있게 합니다"
                          : "편집 중인 칸이 없습니다"
                      }
                      onClick={() => {
                        setMenu(null);
                        void run(() => api.participantUnlock(p.name));
                      }}
                    >
                      편집 잠금 해제
                    </button>
                    <button
                      role="menuitem"
                      className="danger-text"
                      disabled={p.status === "offline"}
                      onClick={() => {
                        setMenu(null);
                        void run(() => api.participantKick(p.name));
                      }}
                    >
                      연결 끊기
                    </button>
                  </div>
                )}
              </span>
            </div>
          );
        })}
      </div>
      <div className="muted small">미접속자에게 안내문을 다시 보내려면 주소·안내문을 복사해 메신저로 공유합니다.</div>
    </section>
  );
}
