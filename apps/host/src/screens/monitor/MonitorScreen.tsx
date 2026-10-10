import type { CellDecoration, GridSelection, SheetGridHandle } from "@mm/grid";
import { filterOptions, rowsNotMatching, SheetGrid, withHiddenRows } from "@mm/grid";
import type {
  Activity,
  BatchView,
  CellRange,
  CellRef,
  CellValue,
  ChatMessage,
  ChatTo,
  CommsSnapshot,
  HighlightAudience,
  HostEvent,
  HostSnapshot,
  MonitorData,
  NoticeTarget,
  ProgressStats,
} from "@mm/shared";
import { a1, colLetters, decodeFlags, displayValue, FLAG, personColor, remaining, splitCellRefs } from "@mm/shared";
import { listen } from "@tauri-apps/api/event";
import type { ReactNode } from "react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import { api, errorText } from "../../api";

type Panel = "activity" | "notice" | "highlight" | "chat";

interface Props {
  active: boolean;
  snapshot: HostSnapshot | null;
  comms: CommsSnapshot | null;
  running: boolean;
  /** 마지막으로 본 채팅 번호(안 읽은 수 계산). */
  chatSeen: number;
  onChatSeen: (id: number) => void;
  onGoHistory: () => void;
}

interface Filter {
  values: Record<number, string>;
  missingOnly: boolean;
  recentDeleted: boolean;
  /** 이 사람(명단 번호)의 행만. */
  person: number | null;
  /** 이 열의 필수 미입력 행만. */
  column: number | null;
}

const NO_FILTER: Filter = { values: {}, missingOnly: false, recentDeleted: false, person: null, column: null };

export function time(ms: number, seconds = true): string {
  const d = new Date(ms);
  const p = (n: number) => String(n).padStart(2, "0");
  return seconds
    ? `${p(d.getHours())}:${p(d.getMinutes())}:${p(d.getSeconds())}`
    : `${p(d.getHours())}:${p(d.getMinutes())}`;
}

export function showValue(v: CellValue | undefined): string {
  if (v === undefined || v === null || v === "") return "(빈 값)";
  if (typeof v === "number") return v.toLocaleString(undefined, { maximumFractionDigits: 6 });
  if (typeof v === "boolean") return v ? "TRUE" : "FALSE";
  if (typeof v === "object") return `#오류(${v.err})`;
  return v;
}

const ORIGIN_LABEL: Record<string, string> = {
  edit: "변경",
  paste: "붙여넣기",
  clear: "범위 삭제",
  undo: "실행 취소",
  redo: "다시 실행",
  revert: "되돌림",
  restore: "스냅샷 복원",
  excel: "Excel 값 유지",
};

/** 채팅 글: `#F7` 같은 셀 참조는 눌러서 그 칸으로 간다(HTML은 해석하지 않는다). */
export function ChatText({ text, onRef }: { text: string; onRef: (r: CellRange) => void }) {
  return (
    <>
      {splitCellRefs(text).map((p, i) =>
        "ref" in p ? (
          <button key={i} className="cell-ref" onClick={() => onRef(p.range)}>
            #{p.ref}
          </button>
        ) : (
          <span key={i}>{p.text}</span>
        ),
      )}
    </>
  );
}

/** 4 진행 모니터링(1d) + 소통 패널(1i 호스트 쪽). */
export function MonitorScreen({ active, snapshot, comms, running, chatSeen, onChatSeen, onGoHistory }: Props) {
  const [data, setData] = useState<MonitorData | null>(null);
  const [, setRev] = useState(0);
  const [progress, setProgress] = useState<ProgressStats | null>(null);
  const [activity, setActivity] = useState<Activity | null>(null);
  const [selection, setSelection] = useState<GridSelection | null>(null);
  const [panel, setPanel] = useState<Panel>("activity");
  const [filter, setFilter] = useState<Filter>(NO_FILTER);
  const [status, setStatus] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [now, setNow] = useState(() => Date.now());
  const grid = useRef<SheetGridHandle>(null);
  const refreshTimer = useRef<number | undefined>(undefined);

  const refresh = useCallback(() => {
    void api
      .monitorProgress()
      .then(setProgress)
      .catch(() => {});
    void api
      .monitorActivity(40)
      .then(setActivity)
      .catch(() => {});
  }, []);

  const load = useCallback(async () => {
    try {
      const d = await api.monitorGet();
      setData(d);
      setProgress(d.view.progress);
      refresh();
    } catch (e) {
      setError(errorText(e));
    }
  }, [refresh]);

  useEffect(() => {
    if (active) void load();
  }, [active, load]);

  // 셀 변경: 바뀐 칸만 고쳐 그린다. 진행률·활동은 잠깐 모았다가 새로 읽는다.
  useEffect(() => {
    const off = listen<HostEvent>("sheet-changes", (e) => {
      const ev = e.payload;
      if (ev.type === "reset") {
        void load();
        return;
      }
      setData((d) => {
        if (!d) return d;
        const m = d.model;
        if (ev.type === "cells") {
          for (const u of ev.cells) {
            const cell = m.cells[u.row * m.cols + u.col];
            if (cell) cell.v = u.value ?? null;
          }
          return d;
        }
        const model = { ...m, styles: ev.styles };
        for (const ch of ev.changes) {
          const cell = model.cells[ch.row * model.cols + ch.col];
          if (cell) cell.s = ch.s;
        }
        return { ...d, model };
      });
      setRev((r) => r + 1);
      window.clearTimeout(refreshTimer.current);
      refreshTimer.current = window.setTimeout(refresh, 400);
    });
    return () => void off.then((f) => f());
  }, [load, refresh]);

  useEffect(() => {
    if (!status) return;
    const id = window.setTimeout(() => setStatus(null), 5000);
    return () => window.clearTimeout(id);
  }, [status]);

  // 마감 카운트다운
  useEffect(() => {
    const id = window.setInterval(() => setNow(Date.now()), 30_000);
    return () => window.clearInterval(id);
  }, []);

  const flags = useMemo(() => (data ? decodeFlags(data.view.flags) : new Uint8Array()), [data]);
  const model = data?.model;
  const view = data?.view;
  const roster = view?.roster ?? [];
  const labels = view?.labels ?? [];
  const columns = view?.config.columns ?? [];
  const filterCols = useMemo(() => columns.map((c, i) => (c.filter ? i : -1)).filter((i) => i >= 0), [columns]);
  const options = useMemo(
    () => (model ? new Map(filterCols.map((c) => [c, filterOptions(model, c)])) : new Map<number, string[]>()),
    [filterCols, model],
  );

  const isEmpty = (v: CellValue | undefined) => v === undefined || v === null || v === "";

  // 필터는 걸 때 계산한다(입력이 들어와도 행이 바로 사라지지 않게).
  const hidden = useMemo(() => {
    const out = new Set<number>();
    if (!model || !view) return out;
    for (const r of rowsNotMatching(model, filter.values)) out.add(r);
    const deleted = new Set(activity?.deletedRows ?? []);
    for (let r = model.headerRows; r < model.rows; r++) {
      if (filter.person !== null && view.rowOwner[r] !== filter.person) out.add(r);
      if (filter.recentDeleted && !deleted.has(r)) out.add(r);
      const missingIn = (c: number) => {
        const i = r * model.cols + c;
        return (flags[i]! & FLAG.REQUIRED) !== 0 && isEmpty(model.cells[i]?.v);
      };
      if (filter.column !== null && !missingIn(filter.column)) out.add(r);
      if (filter.missingOnly) {
        let any = false;
        for (let c = 0; c < model.cols && !any; c++) any = missingIn(c);
        if (!any) out.add(r);
      }
    }
    return out;
    // 값이 바뀔 때는 다시 계산하지 않는다(activity는 '최근 삭제' 필터를 걸 때의 것을 쓴다).
  }, [model, view, flags, filter]);

  const shown = model ? model.rows - model.headerRows - [...hidden].filter((r) => r >= model.headerRows).length : 0;
  const viewModel = useMemo(() => (model ? withHiddenRows(model, hidden) : null), [model, hidden]);
  const filtering =
    filter.missingOnly ||
    filter.recentDeleted ||
    filter.person !== null ||
    filter.column !== null ||
    Object.keys(filter.values).length > 0;

  const origin = model?.origin ?? { row: 0, col: 0 };
  const addr = (at: CellRef) => a1(origin.row + at.row, origin.col + at.col);
  const rangeLabel = (r: CellRange) =>
    r.rows === 1 && r.cols === 1
      ? addr(r)
      : `${addr(r)}:${a1(origin.row + r.row + r.rows - 1, origin.col + r.col + r.cols - 1)}`;
  const colorOf = (name: string) => {
    const i = roster.indexOf(name);
    return i >= 0 ? personColor(i) : "#222";
  };

  /** 그 칸으로 간다(필터에 걸려 숨었으면 필터를 지운다). */
  const reveal = (r: CellRange) => {
    if (hidden.has(r.row)) setFilter(NO_FILTER);
    requestAnimationFrame(() => grid.current?.reveal({ row: r.row, col: r.col }));
  };
  /** 시트 주소 범위(채팅 #참조) → 모델 좌표로 옮겨서 간다. */
  const revealSheet = (r: CellRange) =>
    reveal({ row: r.row - origin.row, col: r.col - origin.col, rows: r.rows, cols: r.cols });

  // ------------------------------------------------------------ 그리드 장식
  const editing = useMemo(() => {
    const map = new Map<string, string>();
    for (const p of snapshot?.participants ?? []) if (p.editing) map.set(`${p.editing.row}:${p.editing.col}`, p.name);
    return map;
  }, [snapshot]);
  const cursors = useMemo(() => {
    const map = new Map<string, string>();
    for (const p of snapshot?.participants ?? [])
      if (p.cursor && p.status !== "offline") map.set(`${p.cursor.row}:${p.cursor.col}`, p.name);
    return map;
  }, [snapshot]);
  const highlighted = useMemo(() => {
    const set = new Set<string>();
    for (const h of comms?.highlights ?? []) {
      if (!h.active) continue;
      for (let r = h.range.row; r < h.range.row + h.range.rows; r++)
        for (let c = h.range.col; c < h.range.col + h.range.cols; c++) set.add(`${r}:${c}`);
    }
    return set;
  }, [comms]);

  const decorate = (r: number, c: number): CellDecoration | undefined => {
    if (!model) return undefined;
    const i = r * model.cols + c;
    const f = flags[i] ?? 0;
    const key = `${r}:${c}`;
    const cls: string[] = [];
    if (f & FLAG.HEADER) return undefined;
    if (!(f & FLAG.INPUT)) cls.push("deco-locked");
    if (f & FLAG.REQUIRED && isEmpty(model.cells[i]?.v)) cls.push("deco-missing");
    if (highlighted.has(key)) cls.push("deco-highlight");
    const editor = editing.get(key);
    const cursor = cursors.get(key);
    if (editor) {
      const color = colorOf(editor);
      cls.push("deco-peer");
      return {
        className: cls.join(" "),
        style: { boxShadow: `inset 0 0 0 2px ${color}` },
        adornment: (
          <span className="peer-tag" style={{ background: color }}>
            {editor} · 잠금
          </span>
        ),
      };
    }
    if (cursor) {
      const color = colorOf(cursor);
      cls.push("deco-peer");
      return {
        className: cls.join(" "),
        style: { boxShadow: `inset 0 0 0 2px ${color}` },
        adornment: (
          <span className="peer-tag" style={{ background: color }}>
            {cursor}
          </span>
        ),
      };
    }
    return { className: cls.join(" ") };
  };

  const cellText = (_r: number, c: number, cell: { v?: unknown } | undefined, nf: string) => {
    const format = columns[c]?.format;
    return format?.kind === "date" ? displayValue(cell?.v as never, nf, format) : undefined;
  };

  // ------------------------------------------------------------ 동작
  const run = async (f: () => Promise<unknown>, done?: string) => {
    setError(null);
    try {
      await f();
      if (done) setStatus(done);
    } catch (e) {
      setError(errorText(e));
    }
  };

  const revertBatch = async (b: BatchView) => {
    let r = await api.historyRevertBatch(b.batch, false);
    if (r.changed === 0 && r.stale.length > 0) {
      const ok = window.confirm(
        `그 뒤에 다시 바뀐 칸 ${r.stale.length}개(${r.stale.map(addr).join(", ")})도 덮어씁니다. 되돌릴까요?`,
      );
      if (!ok) return;
      r = await api.historyRevertBatch(b.batch, true);
    }
    setStatus(`${rangeLabel(b.range)} 묶음을 되돌렸습니다 (${r.changed}칸)`);
  };

  const p = progress;
  const pct = p && p.required > 0 ? Math.round((p.filled / p.required) * 100) : 0;
  const left = remaining(view?.config.deadline, new Date(now));
  const people = snapshot?.participants ?? [];
  const unread = (comms?.chat ?? []).filter((m) => !m.host && m.id > chatSeen).length;
  const sel = selection
    ? {
        row: Math.min(selection.anchor.row, selection.focus.row),
        col: Math.min(selection.anchor.col, selection.focus.col),
        rows: Math.abs(selection.anchor.row - selection.focus.row) + 1,
        cols: Math.abs(selection.anchor.col - selection.focus.col) + 1,
      }
    : null;

  if (!model || !view || !viewModel) {
    return (
      <div className="screen monitor-screen">
        <div className="muted">{error ?? "불러오는 중…"}</div>
      </div>
    );
  }

  return (
    <div className="screen monitor-screen">
      <div className="mon-head">
        <span className="stat">
          <b>
            {snapshot?.online ?? 0}/{snapshot?.roster ?? people.length}
          </b>
          명 접속
        </span>
        <span className="stat" aria-label="전체 필수 진행률">
          <b>
            {(p?.filled ?? 0).toLocaleString()}/{(p?.required ?? 0).toLocaleString()}
          </b>
          필수 칸 채움 <span className="muted">{pct}%</span>
        </span>
        {left && (
          <span className={`stat${left === "마감 지남" ? " warn" : ""}`}>
            {left === "마감 지남" ? (
              <b>{left}</b>
            ) : (
              <>
                <b>{left}</b>
                마감까지
              </>
            )}
          </span>
        )}
        {/* 공유 중인지는 위쪽 상태 알약이 보여 준다. 마감·중지만 여기서 알린다. */}
        {(snapshot?.closed || !running) && (
          <span className={`pill${snapshot?.closed ? " warn-pill" : ""}`}>
            {snapshot?.closed ? "취합 마감됨" : "공유 중지됨"}
          </span>
        )}
        <span className="spacer" />
        <button className="ghost small" onClick={() => setPanel("notice")}>
          공지 보내기
        </button>
        <button className="ghost small" onClick={() => setPanel("highlight")}>
          선택 셀 강조
        </button>
        <button className="ghost small" onClick={() => setPanel("chat")}>
          채팅{unread > 0 ? ` ${unread}` : ""}
        </button>
        <button className="ghost small" onClick={() => void run(() => api.snapshotTake(""), "스냅샷을 남겼습니다")}>
          스냅샷
        </button>
      </div>

      <div className="filter-bar">
        {filterCols.length > 0 && <span className="tag tag-filter">조회조건</span>}
        {filterCols.map((c) => (
          <label key={c} className="filter">
            {labels[c] || colLetters(c)}:
            <select
              aria-label={`${labels[c]} 조회조건`}
              value={filter.values[c] ?? ""}
              onChange={(e) => {
                const values = { ...filter.values };
                if (e.target.value) values[c] = e.target.value;
                else delete values[c];
                setFilter({ ...filter, values });
              }}
            >
              <option value="">전체</option>
              {options.get(c)?.map((o) => (
                <option key={o} value={o}>
                  {o}
                </option>
              ))}
            </select>
          </label>
        ))}
        <label className="check">
          <input
            type="checkbox"
            checked={filter.missingOnly}
            onChange={(e) => setFilter({ ...filter, missingOnly: e.target.checked })}
          />
          필수 미입력만
        </label>
        <label className="check">
          <input
            type="checkbox"
            checked={filter.recentDeleted}
            onChange={(e) => setFilter({ ...filter, recentDeleted: e.target.checked })}
          />
          최근 삭제된 셀
        </label>
        {filter.person !== null && <span className="tag">담당: {roster[filter.person]}</span>}
        {filter.column !== null && <span className="tag">{labels[filter.column]} 미입력</span>}
        {filtering && (
          <button className="link" onClick={() => setFilter(NO_FILTER)}>
            필터 지우기
          </button>
        )}
        <span className="spacer" />
        <span className="muted small" data-testid="mon-rows">
          {(model.rows - model.headerRows).toLocaleString()}행 중 {shown.toLocaleString()}행
        </span>
      </div>

      <div className="mon-body">
        <aside className="mon-left">
          <section aria-label="담당자별">
            <b>담당자별</b>
            {people.map((x) => {
              const pp = p?.byPerson[x.color];
              const ratio = pp && pp.required > 0 ? Math.round((pp.filled / pp.required) * 100) : 0;
              const symbol = x.status === "online" ? "●" : x.status === "away" ? "◐" : "○";
              return (
                <button
                  key={x.name}
                  className={`mon-person${filter.person === x.color ? " on" : ""}`}
                  onClick={() => setFilter({ ...NO_FILTER, person: filter.person === x.color ? null : x.color })}
                  title="누르면 이 사람 행만 봅니다"
                >
                  <span style={{ color: personColor(x.color) }}>{symbol}</span>
                  <span className="grow">{x.name}</span>
                  <span className="mono small">{ratio}%</span>
                </button>
              );
            })}
          </section>
          <section aria-label="열별 완료">
            <b>열별 완료</b>
            {(p?.byColumn ?? [])
              .filter((c) => c.required > 0)
              .map((c) => {
                const ratio = Math.round((c.filled / c.required) * 100);
                return (
                  <button
                    key={c.col}
                    className={`mon-col${filter.column === c.col ? " on" : ""}`}
                    onClick={() => setFilter({ ...NO_FILTER, column: filter.column === c.col ? null : c.col })}
                    title="누르면 이 열의 필수 미입력 행만 봅니다"
                  >
                    <span className="mono">{colLetters(origin.col + c.col)}</span>
                    <span className="grow">{labels[c.col]}</span>
                    <span className="mono small">{ratio}%</span>
                  </button>
                );
              })}
          </section>
        </aside>

        <div className="mon-grid">
          <SheetGrid
            handle={grid}
            model={viewModel}
            decorate={decorate}
            cellText={cellText}
            rowMarker={(r) => {
              const o = view.rowOwner[r] ?? -1;
              return o >= 0 ? personColor(o) : undefined;
            }}
            selection={selection}
            onSelectionChange={setSelection}
          />
        </div>

        <aside className="mon-right">
          <div className="mon-tabs" role="tablist">
            {(
              [
                ["activity", "실시간 활동"],
                ["notice", "공지"],
                ["highlight", "강조"],
                ["chat", `채팅${unread > 0 ? ` ${unread}` : ""}`],
              ] as const
            ).map(([k, label]) => (
              <button key={k} role="tab" aria-selected={panel === k} onClick={() => setPanel(k)}>
                {label}
              </button>
            ))}
          </div>
          {status && (
            <div className="ok small mon-status" role="status">
              {status}
            </div>
          )}
          {error && <div className="error small">{error}</div>}
          {panel === "activity" && (
            <ActivityPanel
              activity={activity}
              snapshot={snapshot}
              addr={addr}
              rangeLabel={rangeLabel}
              colorOf={colorOf}
              labels={labels}
              onReveal={reveal}
              onRevert={(b) => void run(() => revertBatch(b))}
              onGoHistory={onGoHistory}
            />
          )}
          {panel === "notice" && <NoticePanel comms={comms} snapshot={snapshot} onError={setError} />}
          {panel === "highlight" && (
            <HighlightPanel
              comms={comms}
              selection={sel}
              ownerNames={sel ? ownersOf(sel, view.rowOwner, roster) : []}
              rangeLabel={rangeLabel}
              onReveal={reveal}
              onError={setError}
            />
          )}
          {panel === "chat" && (
            <ChatPanel
              comms={comms}
              roster={roster}
              colorOf={colorOf}
              onRef={revealSheet}
              onSeen={onChatSeen}
              onError={setError}
            />
          )}
        </aside>
      </div>
    </div>
  );
}

function ownersOf(r: CellRange, rowOwner: number[], roster: string[]): string[] {
  const out = new Set<string>();
  for (let row = r.row; row < r.row + r.rows; row++) {
    const o = rowOwner[row] ?? -1;
    if (o >= 0 && roster[o]) out.add(roster[o]!);
  }
  return [...out];
}

// ------------------------------------------------------------ 패널들

function ActivityPanel(props: {
  activity: Activity | null;
  snapshot: HostSnapshot | null;
  addr: (at: CellRef) => string;
  rangeLabel: (r: CellRange) => string;
  colorOf: (name: string) => string;
  labels: string[];
  onReveal: (r: CellRange) => void;
  onRevert: (b: BatchView) => void;
  onGoHistory: () => void;
}) {
  const { activity, snapshot, addr, rangeLabel, colorOf, labels, onReveal, onRevert, onGoHistory } = props;
  const editing = (snapshot?.participants ?? []).filter((p) => p.editing);
  return (
    <div className="feed" aria-label="실시간 활동">
      {editing.map((p) => (
        <div key={`e${p.name}`} className="feed-item editing">
          <span className="dot" style={{ background: personColor(p.color) }} />
          {p.name} <span className="mono">{addr(p.editing!)}</span> 입력 중… <span className="tag">잠금</span>
        </div>
      ))}
      {(activity?.page.batches ?? []).map((b) => {
        const first = b.entries[0]!;
        return (
          <div key={`${b.batch}:${b.atMs}`} className={`feed-item${b.alert ? " alert" : ""}`}>
            <div>
              <span className="muted small">{time(b.atMs)}</span> ·{" "}
              <span className="dot" style={{ background: colorOf(b.user) }} />
              {b.user}
              {b.count === 1 && b.origin === "edit" ? (
                <>
                  {" "}
                  <span className="mono">{addr(first)}</span> {showValue(first.old)} → <b>{showValue(first.new)}</b>
                </>
              ) : (
                <>
                  {" "}
                  ·{" "}
                  <b>
                    {ORIGIN_LABEL[b.origin] ?? "변경"} {b.count}셀
                  </b>{" "}
                  <span className="mono">{rangeLabel(b.range)}</span>
                  <div className="small muted">
                    {b.entries
                      .slice(0, 3)
                      .map((e) => `${labels[e.col] ?? ""} ${showValue(e.old)} → ${showValue(e.new)}`)
                      .join(" · ")}
                    {b.entries.length > 3 ? " …" : ""}
                  </div>
                </>
              )}
            </div>
            {(b.alert || b.count > 1) && (
              <div className="feed-actions">
                <button className="ghost small" onClick={() => onRevert(b)}>
                  되돌리기
                </button>
                <button className="ghost small" onClick={() => onReveal(b.range)}>
                  셀로 이동
                </button>
              </div>
            )}
          </div>
        );
      })}
      {(activity?.page.batches.length ?? 0) === 0 && editing.length === 0 && (
        <div className="muted small">아직 입력이 없습니다.</div>
      )}
      <button className="link" onClick={onGoHistory}>
        전체 히스토리 보기 →
      </button>
    </div>
  );
}

function NoticePanel({
  comms,
  snapshot,
  onError,
}: {
  comms: CommsSnapshot | null;
  snapshot: HostSnapshot | null;
  onError: (e: string) => void;
}) {
  const [kind, setKind] = useState<NoticeTarget["kind"]>("all");
  const [picked, setPicked] = useState<string[]>([]);
  const [important, setImportant] = useState(false);
  const [text, setText] = useState("");
  const people = snapshot?.participants ?? [];
  const incomplete = people.filter((p) => p.filled < p.required).length;

  const send = async (target: NoticeTarget, body: string, imp: boolean) => {
    try {
      await api.noticeSend(body, imp, target);
      setText("");
    } catch (e) {
      onError(errorText(e));
    }
  };
  const target: NoticeTarget = kind === "people" ? { kind: "people", names: picked } : { kind };

  return (
    <div className="panel-body" aria-label="공지 보내기">
      <div className="chips">
        <button className={`chip${kind === "all" ? " on" : ""}`} onClick={() => setKind("all")}>
          전체 {people.length}
        </button>
        <button className={`chip${kind === "incomplete" ? " on" : ""}`} onClick={() => setKind("incomplete")}>
          미완료자 {incomplete}
        </button>
        <button className={`chip${kind === "people" ? " on" : ""}`} onClick={() => setKind("people")}>
          선택…
        </button>
        <label className="check">
          <input type="checkbox" checked={important} onChange={(e) => setImportant(e.target.checked)} />
          중요
        </label>
      </div>
      {kind === "people" && (
        <div className="pick-people">
          {people.map((p) => (
            <label key={p.name} className="check">
              <input
                type="checkbox"
                checked={picked.includes(p.name)}
                onChange={(e) => setPicked(e.target.checked ? [...picked, p.name] : picked.filter((n) => n !== p.name))}
              />
              {p.name}
            </label>
          ))}
        </div>
      )}
      <textarea
        aria-label="공지 내용"
        rows={3}
        placeholder="예: 오늘 17시까지 산출 근거 입력 부탁드립니다."
        value={text}
        onChange={(e) => setText(e.target.value)}
      />
      <button
        disabled={!text.trim() || (kind === "people" && picked.length === 0)}
        onClick={() => void send(target, text, important)}
      >
        공지 보내기
      </button>
      <div className="sent">
        <b className="small">보낸 공지</b>
        {[...(comms?.notices ?? [])].reverse().map((n) => {
          const unread = n.recipients.filter((r) => !n.read.includes(r));
          return (
            <div key={n.id} className="sent-item">
              <div>
                <span className="muted small">{time(n.atMs, false)}</span>{" "}
                {n.important && <span className="tag tag-required">중요</span>} {n.text}
              </div>
              <div className="small muted">
                읽음 {n.read.length}/{n.recipients.length}
                {unread.length > 0 && (
                  <button
                    className="link"
                    onClick={() => void send({ kind: "people", names: unread }, n.text, n.important)}
                  >
                    안 읽은 {unread.length}명에게 다시 보내기
                  </button>
                )}
              </div>
            </div>
          );
        })}
      </div>
    </div>
  );
}

function HighlightPanel(props: {
  comms: CommsSnapshot | null;
  selection: CellRange | null;
  ownerNames: string[];
  rangeLabel: (r: CellRange) => string;
  onReveal: (r: CellRange) => void;
  onError: (e: string) => void;
}) {
  const { comms, selection, ownerNames, rangeLabel, onReveal, onError } = props;
  const [note, setNote] = useState("");
  const [audience, setAudience] = useState<HighlightAudience>("owners");
  const add = async () => {
    if (!selection) return;
    try {
      await api.highlightAdd(selection, note, audience);
      setNote("");
    } catch (e) {
      onError(errorText(e));
    }
  };
  return (
    <div className="panel-body" aria-label="셀 강조">
      <div className="small">
        그리드에서 선택한 셀 <b className="mono">{selection ? rangeLabel(selection) : "— 그리드에서 셀을 고르세요"}</b>
      </div>
      <textarea
        aria-label="강조 메모"
        rows={2}
        placeholder="예: 교육 과정명과 인원을 적어 주세요."
        value={note}
        onChange={(e) => setNote(e.target.value)}
      />
      <div className="small">
        보일 사람{" "}
        <label className="radio-inline">
          <input type="radio" checked={audience === "owners"} onChange={() => setAudience("owners")} />
          담당자{ownerNames.length > 0 ? `(${ownerNames.join(", ")})` : ""}
        </label>
        <label className="radio-inline">
          <input type="radio" checked={audience === "all"} onChange={() => setAudience("all")} />
          모두
        </label>
      </div>
      <button disabled={!selection} onClick={() => void add()}>
        강조 표시
      </button>
      <div className="sent">
        {[...(comms?.highlights ?? [])]
          .filter((h) => h.active)
          .reverse()
          .map((h) => (
            <div key={h.id} className="sent-item">
              <div>
                <button className="cell-ref" onClick={() => onReveal(h.range)}>
                  {rangeLabel(h.range)}
                </button>{" "}
                {h.note}
              </div>
              <div className="small">
                {h.recipients.length === 0 && <span className="warn">보일 사람이 없습니다(담당자 없음)</span>}
                {h.recipients.map((r) => {
                  const ack = h.acked.find(([n]) => n === r);
                  return (
                    <span key={r} className={ack ? "ok" : "muted"}>
                      {r} · {ack ? `확인함 ${time(ack[1], false)}` : "미확인"}{" "}
                    </span>
                  );
                })}
                <button
                  className="ghost small"
                  onClick={() => void api.highlightClear(h.id).catch((e) => onError(errorText(e)))}
                >
                  해제
                </button>
              </div>
            </div>
          ))}
      </div>
    </div>
  );
}

function chatTarget(m: ChatMessage): ReactNode {
  if (m.to.kind === "host") return <span className="muted small"> → 호스트</span>;
  if (m.to.kind === "person") return <span className="muted small"> → {m.to.name}</span>;
  return null;
}

function ChatPanel(props: {
  comms: CommsSnapshot | null;
  roster: string[];
  colorOf: (name: string) => string;
  onRef: (r: CellRange) => void;
  onSeen: (id: number) => void;
  onError: (e: string) => void;
}) {
  const { comms, roster, colorOf, onRef, onSeen, onError } = props;
  const [to, setTo] = useState("");
  const [text, setText] = useState("");
  const list = useRef<HTMLDivElement>(null);
  const messages = comms?.chat ?? [];
  const last = messages[messages.length - 1]?.id ?? 0;
  useEffect(() => {
    onSeen(last);
    list.current?.scrollTo({ top: list.current.scrollHeight });
  }, [last, onSeen]);
  const send = async () => {
    const target: ChatTo = to ? { kind: "person", name: to } : { kind: "all" };
    try {
      await api.chatSend(target, text);
      setText("");
    } catch (e) {
      onError(errorText(e));
    }
  };
  return (
    <div className="panel-body chat" aria-label="채팅">
      <div className="chat-list" ref={list}>
        {messages.length === 0 && <div className="muted small">아직 채팅이 없습니다.</div>}
        {messages.map((m) => (
          <div key={m.id} className={`chat-msg${m.host ? " mine" : ""}`}>
            <div className="small">
              <b style={{ color: m.host ? "#222" : colorOf(m.from) }}>{m.host ? "나(호스트)" : m.from}</b>
              {chatTarget(m)} <span className="muted">· {time(m.atMs, false)}</span>
            </div>
            <div>
              <ChatText text={m.text} onRef={onRef} />
            </div>
          </div>
        ))}
      </div>
      <div className="chat-input">
        <select aria-label="받는 사람" value={to} onChange={(e) => setTo(e.target.value)}>
          <option value="">전체</option>
          {roster.map((n) => (
            <option key={n} value={n}>
              {n} (1:1)
            </option>
          ))}
        </select>
        <input
          aria-label="채팅 입력"
          placeholder="메시지 · #셀 참조(예: #F7)"
          value={text}
          onChange={(e) => setText(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && !e.nativeEvent.isComposing && text.trim()) void send();
          }}
        />
        <button className="small" disabled={!text.trim()} onClick={() => void send()}>
          전송
        </button>
      </div>
    </div>
  );
}
