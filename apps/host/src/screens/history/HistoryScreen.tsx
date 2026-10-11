import { cellText, effectiveColumn } from "@mm/grid";
import type {
  BatchView,
  CellRange,
  CellRef,
  ChangeEntry,
  ChangeKind,
  HistoryFilter,
  HistoryPage,
  MonitorData,
  SheetCell,
  SheetModel,
  SnapshotDiff,
  SnapshotView,
  Timeline,
} from "@mm/shared";
import { parseSheetRange, personColor } from "@mm/shared";
import { save as saveDialog } from "@tauri-apps/plugin-dialog";
import { listen } from "@tauri-apps/api/event";
import { useCallback, useEffect, useMemo, useState } from "react";

import { errorText, jobEvent } from "../../api";
import { useApi, useJobId } from "../../job";
import { sheetAddr, showValue, time } from "../monitor/MonitorScreen";

const PAGE = 50;

const KINDS: { kind: ChangeKind; label: string }[] = [
  { kind: "input", label: "입력" },
  { kind: "edit", label: "수정" },
  { kind: "delete", label: "삭제" },
  { kind: "revert", label: "되돌림" },
];

const KIND_LABEL: Record<ChangeKind, string> = { input: "입력", edit: "수정", delete: "삭제", revert: "되돌림" };

const SNAPSHOT_LABEL: Record<string, string> = {
  original: "불러온 원본",
  auto: "자동 (15분)",
  manual: "수동",
  beforeRestore: "복원 직전",
};

const BATCH_LABEL: Record<string, string> = {
  paste: "붙여넣기",
  clear: "범위 삭제",
  undo: "실행 취소",
  redo: "다시 실행",
  revert: "되돌림",
  restore: "스냅샷 복원",
  excel: "Excel 값 유지",
  addRow: "행 추가",
  edit: "변경",
};

type Period = "today" | "hour" | "all";

function since(period: Period): number | undefined {
  if (period === "all") return undefined;
  if (period === "hour") return Date.now() - 3_600_000;
  const d = new Date();
  return new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime();
}

interface Props {
  active: boolean;
  /** 확정한 시트들(세션 안 순번 순서). */
  sheets: SheetModel[];
  /** 1b·1d에서 보던 시트: 시트 이름 없이 적은 셀/범위의 시트. */
  sheet: number;
}

/** 5 히스토리(1h): 변경 기록(묶음)·필터·CSV, 셀 타임라인, 되돌리기, 스냅샷. 기록은 모든 시트, 시트로 거를 수 있다. */
export function HistoryScreen({ active, sheets, sheet }: Props) {
  const api = useApi();
  const job = useJobId();
  /** 시트마다 지금 값·열 이름·명단(행 이름·항목 표시용). */
  const [data, setData] = useState<(MonitorData | undefined)[]>([]);
  /** 이 시트의 기록만(없으면 모든 시트). */
  const [only, setOnly] = useState<number | null>(null);
  const [period, setPeriod] = useState<Period>("today");
  const [user, setUser] = useState("");
  const [kinds, setKinds] = useState<ChangeKind[]>([]);
  const [rangeText, setRangeText] = useState("");
  const [text, setText] = useState("");
  const [page, setPage] = useState<HistoryPage | null>(null);
  const [batches, setBatches] = useState<BatchView[]>([]);
  const [cell, setCell] = useState<SheetCell | null>(null);
  const [timeline, setTimeline] = useState<Timeline | null>(null);
  const [snapshots, setSnapshots] = useState<SnapshotView[]>([]);
  const [picked, setPicked] = useState<number | null>(null);
  const [diff, setDiff] = useState<SnapshotDiff | null>(null);
  const [confirm, setConfirm] = useState<{ message: string; run: () => Promise<void> } | null>(null);
  const [status, setStatus] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const many = sheets.length > 1;
  // 명단은 모든 시트의 합집합이라 어느 시트 것이든 같다.
  const roster = data.find(Boolean)?.view.roster ?? [];
  const labelsOf = (s: number) => data[s]?.view.labels ?? [];
  const addr = (s: number, at: CellRef | CellRange) => sheetAddr(sheets, s, at);

  // 범위 글자(`F7:H7`, `인력계획!F7`) → 그 시트의 모델 좌표. 시트 이름이 없으면 고른 시트(없으면 보던 시트). 틀리면 필터 없음.
  const target = useMemo(() => {
    const p = parseSheetRange(rangeText);
    if (!p) return undefined;
    const s =
      p.sheet === undefined
        ? (only ?? sheet)
        : sheets.findIndex((m) => m.sheet.toUpperCase() === p.sheet!.toUpperCase());
    const o = sheets[s]?.origin;
    if (!o) return undefined;
    const r = p.range;
    return { sheet: s, range: { row: r.row - o.row, col: r.col - o.col, rows: r.rows, cols: r.cols } };
  }, [rangeText, only, sheet, sheets]);

  const filter: HistoryFilter = useMemo(
    () => ({
      sinceMs: since(period),
      user: user || undefined,
      sheet: target?.sheet ?? only ?? undefined,
      kinds,
      range: target?.range,
      text: text.trim() || undefined,
    }),
    [period, user, target, only, kinds, text],
  );

  // 행 이름(조회조건 열: '재무팀 · 출장비'). 시트마다.
  const rowLabels = useMemo(
    () =>
      data.map((d) => {
        if (!d) return () => "";
        const model = d.model;
        const cols = d.view.config.columns.map((c, i) => (c.filter ? i : -1)).filter((i) => i >= 0);
        const values = cols.map((c) => ({ c, v: effectiveColumn(model, c) }));
        return (row: number) =>
          values
            .slice(-2)
            .map(({ c, v }) => cellText(model, v[row], row, c))
            .filter(Boolean)
            .join(" · ");
      }),
    [data],
  );
  const rowLabel = (s: number, row: number) => rowLabels[s]?.(row) ?? "";

  const query = useCallback(
    async (offset: number) => {
      try {
        const p = await api.historyQuery(filter, offset, PAGE);
        setPage(p);
        setBatches((b) => (offset === 0 ? p.batches : [...b, ...p.batches]));
      } catch (e) {
        setError(errorText(e));
      }
    },
    [filter],
  );

  const loadSnapshots = useCallback(
    () =>
      void api
        .snapshotList()
        .then(setSnapshots)
        .catch(() => {}),
    [],
  );
  const loadTimeline = useCallback(
    (at: SheetCell) =>
      void api
        .historyTimeline(at.sheet, { row: at.row, col: at.col })
        .then(setTimeline)
        .catch(() => {}),
    [],
  );

  const count = sheets.length;
  useEffect(() => {
    if (!active) return;
    void Promise.all(Array.from({ length: count }, (_, i) => api.monitorGet(i)))
      .then(setData)
      .catch((e) => setError(errorText(e)));
    loadSnapshots();
  }, [active, count, loadSnapshots]);

  useEffect(() => {
    if (active) void query(0);
  }, [active, query]);

  // 새 입력이 들어오면 첫 쪽·타임라인·스냅샷 차이를 새로 읽는다(잠깐 모아서).
  useEffect(() => {
    if (!active) return;
    let timer: number | undefined;
    const off = listen(jobEvent("sheet-changes", job), () => {
      window.clearTimeout(timer);
      timer = window.setTimeout(() => {
        void query(0);
        loadSnapshots();
        if (cell) loadTimeline(cell);
      }, 600);
    });
    return () => {
      window.clearTimeout(timer);
      void off.then((f) => f());
    };
  }, [active, query, loadSnapshots, loadTimeline, cell]);

  useEffect(() => {
    if (cell) loadTimeline(cell);
    else setTimeline(null);
  }, [cell, loadTimeline]);

  useEffect(() => {
    if (!status) return;
    const id = window.setTimeout(() => setStatus(null), 5000);
    return () => window.clearTimeout(id);
  }, [status]);

  const run = async (f: () => Promise<unknown>) => {
    setError(null);
    try {
      await f();
    } catch (e) {
      setError(errorText(e));
    }
  };

  /** 되돌리기: 그 뒤 다시 바뀐 칸이 있으면 먼저 묻는다. */
  const revert = async (
    label: string,
    attempt: (force: boolean) => Promise<{ changed: number; stale: SheetCell[] }>,
  ) => {
    const r = await attempt(false);
    if (r.changed === 0 && r.stale.length > 0) {
      const stale = r.stale
        .slice(0, 5)
        .map((c) => addr(c.sheet, c))
        .join(", ");
      setConfirm({
        message: `그 뒤에 다시 바뀐 칸 ${r.stale.length}개(${stale})도 덮어씁니다.`,
        run: async () => {
          const again = await attempt(true);
          setStatus(`${label}: ${again.changed}칸을 되돌렸습니다`);
        },
      });
      return;
    }
    setStatus(r.changed > 0 ? `${label}: ${r.changed}칸을 되돌렸습니다` : `${label}: 이미 그 값입니다`);
  };

  const exportCsv = async () => {
    const path = await saveDialog({
      defaultPath: `변경이력_${new Date().toISOString().slice(0, 10)}.csv`,
      filters: [{ name: "CSV", extensions: ["csv"] }],
    });
    if (!path) return;
    const n = await api.historyExportCsv(filter, path);
    setStatus(`CSV로 내보냈습니다: ${n.toLocaleString()}건 · ${path}`);
  };

  const userColor = (name: string) => {
    const i = roster.indexOf(name);
    return i >= 0 ? personColor(i) : "#222";
  };

  const entryRow = (e: ChangeEntry, child: boolean) => (
    <div
      key={e.seq}
      className={`hrow${child ? " child" : ""}${cell && cell.sheet === e.sheet && cell.row === e.row && cell.col === e.col ? " on" : ""}`}
      role="row"
      onClick={() => setCell({ sheet: e.sheet, row: e.row, col: e.col })}
    >
      {child ? (
        <>
          <span className="muted">└</span>
          <span />
        </>
      ) : (
        <>
          <span className="mono small">{time(e.atMs)}</span>
          <span>
            <span className="dot" style={{ background: userColor(e.user) }} />
            {e.user}
          </span>
        </>
      )}
      <span className="mono">{addr(e.sheet, e)}</span>
      <span className="small muted">
        {child
          ? labelsOf(e.sheet)[e.col]
          : `${rowLabel(e.sheet, e.row)}${labelsOf(e.sheet)[e.col] ? ` · ${labelsOf(e.sheet)[e.col]}` : ""}`}
      </span>
      <span>
        <span className="muted">{showValue(e.old)}</span> → <b>{showValue(e.new)}</b>{" "}
        <span className={`kind kind-${e.kind}`}>{KIND_LABEL[e.kind]}</span>
      </span>
      <span>
        <button
          className="ghost small"
          onClick={(ev) => {
            ev.stopPropagation();
            void run(() => revert(addr(e.sheet, e), (force) => api.historyRevert([e.seq], force)));
          }}
        >
          되돌리기
        </button>
      </span>
    </div>
  );

  const pickedSnap = snapshots.find((s) => s.meta.id === picked);

  return (
    <div className="screen history-screen">
      <section className="card history-main" aria-label="변경 히스토리">
        <div className="card-head">
          <b className="big">변경 히스토리</b>
          <span className="muted small">
            {page
              ? `${page.totalEntries.toLocaleString()}건 · ${page.totalBatches.toLocaleString()}묶음 · 사용자 ${page.users.length}명`
              : ""}
          </span>
          <span className="spacer" />
          <button className="ghost small" onClick={() => void run(exportCsv)}>
            CSV 내보내기
          </button>
        </div>
        <div className="history-filters">
          <select aria-label="기간" value={period} onChange={(e) => setPeriod(e.target.value as Period)}>
            <option value="today">오늘</option>
            <option value="hour">최근 1시간</option>
            <option value="all">전체</option>
          </select>
          {many && (
            <select
              aria-label="시트"
              value={only ?? ""}
              onChange={(e) => setOnly(e.target.value === "" ? null : Number(e.target.value))}
            >
              <option value="">시트: 전체</option>
              {sheets.map((m, i) => (
                <option key={i} value={i}>
                  {m.sheet}
                </option>
              ))}
            </select>
          )}
          <select aria-label="사용자" value={user} onChange={(e) => setUser(e.target.value)}>
            <option value="">사용자: 전체</option>
            {(page?.users ?? []).map((u) => (
              <option key={u} value={u}>
                {u}
              </option>
            ))}
          </select>
          {KINDS.map((k) => (
            <button
              key={k.kind}
              className={`chip small${kinds.includes(k.kind) ? " on" : ""}`}
              aria-pressed={kinds.includes(k.kind)}
              onClick={() => setKinds(kinds.includes(k.kind) ? kinds.filter((x) => x !== k.kind) : [...kinds, k.kind])}
            >
              {k.label}
            </button>
          ))}
          <input
            aria-label="셀/범위"
            className="mono"
            placeholder={many ? "셀/범위 (예: F7:H7, 인력계획!E5)" : "셀/범위 (예: F7:H7)"}
            value={rangeText}
            onChange={(e) => setRangeText(e.target.value.toUpperCase())}
          />
          <input aria-label="값 검색" placeholder="값 검색" value={text} onChange={(e) => setText(e.target.value)} />
        </div>
        {status && (
          <div className="ok small" role="status">
            {status}
          </div>
        )}
        {error && <div className="error small">{error}</div>}
        {confirm && (
          <div className="confirm" role="alertdialog">
            <span>{confirm.message}</span>
            <button
              className="small"
              onClick={() => {
                const c = confirm;
                setConfirm(null);
                void run(c.run);
              }}
            >
              그래도 되돌리기
            </button>
            <button className="ghost small" onClick={() => setConfirm(null)}>
              취소
            </button>
          </div>
        )}
        <div className="htable" role="table" aria-label="변경 기록">
          <div className="hrow hhead" role="row">
            <span>시간</span>
            <span>사용자</span>
            <span>셀</span>
            <span>행</span>
            <span>이전 → 이후</span>
            <span />
          </div>
          {batches.map((b) =>
            b.entries.length === 1 && b.count === 1 ? (
              entryRow(b.entries[0]!, false)
            ) : (
              <div key={`${b.batch}:${b.atMs}`} className={`hbatch${b.alert ? " alert" : ""}`}>
                <div className="hrow" role="row">
                  <span className="mono small">{time(b.atMs)}</span>
                  <span>
                    <span className="dot" style={{ background: userColor(b.user) }} />
                    {b.user}
                  </span>
                  <span className="mono">{addr(b.sheet, b.range)}</span>
                  <span className="small muted">{rowLabel(b.sheet, b.range.row)}</span>
                  <span>
                    <b className={b.alert ? "error" : ""}>
                      {BATCH_LABEL[b.origin] ?? "변경"} · {b.count}셀
                    </b>
                  </span>
                  <span>
                    <button
                      className="ghost small"
                      onClick={() =>
                        void run(() => revert("묶음 복원", (force) => api.historyRevertBatch(b.batch, force)))
                      }
                    >
                      묶음 복원
                    </button>
                  </span>
                </div>
                {b.entries.map((e) => entryRow(e, true))}
              </div>
            ),
          )}
          {batches.length === 0 && <div className="muted small hempty">조건에 맞는 기록이 없습니다.</div>}
          {page && batches.length < page.totalBatches && (
            <button className="link" onClick={() => void query(batches.length)}>
              더 보기 ({(page.totalBatches - batches.length).toLocaleString()}묶음)
            </button>
          )}
        </div>
      </section>

      <aside className="history-side">
        <section className="card" aria-label="셀 타임라인">
          <b>셀 타임라인{cell ? ` · ${addr(cell.sheet, cell)}` : ""}</b>
          {!timeline && <div className="muted small">기록에서 칸을 누르면 그 칸의 변경이 시간순으로 보입니다.</div>}
          {timeline && (
            <ol className="timeline">
              <li>
                <span className="muted small">원본</span> {showValue(timeline.original)}
                {timeline.entries.length > 0 && timeline.original !== timeline.current && (
                  <button
                    className="link"
                    onClick={() =>
                      void run(async () => {
                        await api.historySetValue(timeline.sheet, timeline.at, timeline.original ?? null);
                        setStatus(`${addr(timeline.sheet, timeline.at)}을(를) 원본 값으로 되돌렸습니다`);
                      })
                    }
                  >
                    이 값으로
                  </button>
                )}
              </li>
              {timeline.entries.map((e, i) => {
                const last = i === timeline.entries.length - 1;
                return (
                  <li key={e.seq}>
                    <span className="muted small">{time(e.atMs, false)}</span> {e.user} {KIND_LABEL[e.kind]}{" "}
                    <b>{showValue(e.new)}</b>
                    {last ? (
                      <span className="tag">현재</span>
                    ) : (
                      <button
                        className="link"
                        onClick={() =>
                          void run(async () => {
                            await api.historySetValue(timeline.sheet, timeline.at, e.new ?? null);
                            setStatus(
                              `${addr(timeline.sheet, timeline.at)}을(를) ${showValue(e.new)}(으)로 바꿨습니다`,
                            );
                          })
                        }
                      >
                        이 값으로
                      </button>
                    )}
                  </li>
                );
              })}
            </ol>
          )}
        </section>

        <section className="card" aria-label="스냅샷">
          <div className="card-head">
            <b>스냅샷</b>
            <span className="spacer" />
            <button
              className="ghost small"
              onClick={() =>
                void run(async () => {
                  await api.snapshotTake("");
                  loadSnapshots();
                  setStatus("스냅샷을 남겼습니다");
                })
              }
            >
              + 지금 저장
            </button>
          </div>
          <ul className="snapshots">
            {snapshots.map((s) => (
              <li key={s.meta.id} className={s.meta.id === picked ? "on" : undefined}>
                <button className="snap-item" onClick={() => setPicked(s.meta.id === picked ? null : s.meta.id)}>
                  <span className="mono small">{time(s.meta.atMs, false)}</span>{" "}
                  {s.meta.label || SNAPSHOT_LABEL[s.meta.kind]}
                </button>
                {s.meta.id === picked && pickedSnap && (
                  <div className="snap-detail small">
                    현재와 {pickedSnap.diff.toLocaleString()}셀 다름
                    {pickedSnap.usersAfter.length > 0 ? ` · 이후 변경 ${pickedSnap.usersAfter.length}명` : ""}
                    <div className="row">
                      <button
                        className="ghost small"
                        disabled={pickedSnap.diff === 0}
                        onClick={() => void run(async () => setDiff(await api.snapshotDiff(s.meta.id)))}
                      >
                        비교 보기
                      </button>
                      <button
                        className="small"
                        disabled={pickedSnap.diff === 0}
                        onClick={() =>
                          setConfirm({
                            message: `${time(s.meta.atMs, false)} 스냅샷으로 ${pickedSnap.diff}칸을 되돌립니다. 지금 상태는 '복원 직전' 스냅샷으로 남습니다.`,
                            run: async () => {
                              const n = await api.snapshotRestore(s.meta.id);
                              loadSnapshots();
                              setStatus(`스냅샷으로 복원했습니다(${n}칸)`);
                            },
                          })
                        }
                      >
                        이 시점으로 복원
                      </button>
                    </div>
                  </div>
                )}
              </li>
            ))}
          </ul>
          <div className="muted small">복원 직전 상태는 자동으로 스냅샷에 남아 복원도 되돌릴 수 있습니다.</div>
        </section>
      </aside>

      {diff && (
        <div className="modal-back" onClick={() => setDiff(null)}>
          <div className="modal card" role="dialog" aria-label="스냅샷 비교" onClick={(e) => e.stopPropagation()}>
            <div className="card-head">
              <b>스냅샷과 지금 비교 · {diff.total.toLocaleString()}칸</b>
              <span className="spacer" />
              <button className="ghost small" onClick={() => setDiff(null)}>
                닫기
              </button>
            </div>
            <div className="diff-table">
              <div className="drow dhead">
                <span>셀</span>
                <span>항목</span>
                <span>스냅샷</span>
                <span>지금</span>
              </div>
              {diff.cells.map((d) => (
                <div key={`${d.sheet}:${d.at.row}:${d.at.col}`} className="drow">
                  <span className="mono">{addr(d.sheet, d.at)}</span>
                  <span className="small muted">{diff.sheets[d.sheet]?.labels[d.at.col]}</span>
                  <span>{showValue(d.snapshot)}</span>
                  <span>
                    <b>{showValue(d.current)}</b>
                  </span>
                </div>
              ))}
              {diff.total > diff.cells.length && (
                <div className="muted small">외 {(diff.total - diff.cells.length).toLocaleString()}칸</div>
              )}
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
