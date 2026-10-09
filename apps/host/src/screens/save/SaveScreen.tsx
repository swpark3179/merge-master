import type {
  AfterSave,
  CellRef,
  CellValue,
  Conflict,
  HostSnapshot,
  SaveCheck,
  SaveInfo,
  SaveOptions,
  SaveResult,
} from "@mm/shared";
import { a1, personColor } from "@mm/shared";
import { save as saveDialog } from "@tauri-apps/plugin-dialog";
import { useCallback, useEffect, useMemo, useState } from "react";

import { api, errorText } from "../../api";

interface Props {
  active: boolean;
  snapshot: HostSnapshot | null;
  /** 서비스를 멈췄을 때(저장 뒤 '서비스 종료') 바깥 상태도 맞춘다. */
  onStopped: () => void;
}

/** 바뀐 칸 표시 색(연한 노랑). */
const MARK = 0xfff3b0;

function show(v: CellValue | undefined): string {
  if (v === undefined || v === null || v === "") return "(빈 칸)";
  if (typeof v === "number") return v.toLocaleString(undefined, { maximumFractionDigits: 6 });
  if (typeof v === "boolean") return v ? "TRUE" : "FALSE";
  if (typeof v === "object") return `#오류(${v.err})`;
  return v;
}

function clock(ms: number): string {
  const d = new Date(ms);
  const p = (n: number) => String(n).padStart(2, "0");
  return `${p(d.getMonth() + 1)}/${p(d.getDate())} ${p(d.getHours())}:${p(d.getMinutes())}`;
}

const key = (at: CellRef) => `${at.row}:${at.col}`;

/** 6 저장·마감: 취합 요약 → 원본과 비교 → 엑셀로 저장 → (선택) 마감. */
export function SaveScreen({ active, snapshot, onStopped }: Props) {
  const [info, setInfo] = useState<SaveInfo | null>(null);
  const [target, setTarget] = useState<"original" | "copy">("copy");
  const [path, setPath] = useState("");
  const [history, setHistory] = useState(true);
  const [mark, setMark] = useState(false);
  const [styles, setStyles] = useState(true);
  const [after, setAfter] = useState<AfterSave>("keep");
  const [check, setCheck] = useState<SaveCheck | null>(null);
  /** 충돌 칸 → 어느 값을 쓸지. */
  const [choice, setChoice] = useState<Record<string, "collected" | "excel">>({});
  const [force, setForce] = useState(false);
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [result, setResult] = useState<SaveResult | null>(null);

  const reload = useCallback(async () => {
    try {
      const i = await api.saveInfo();
      setInfo(i);
      setPath((p) => p || i.defaultCopy);
      if (!i.sourcePath) setTarget("copy");
    } catch (e) {
      setError(errorText(e));
    }
  }, []);

  useEffect(() => {
    if (active) void reload();
  }, [active, reload]);

  // 접속자 표가 바뀌면(입력이 들어오면) 요약도 새로 읽는다(잦으면 1초에 한 번).
  const filled = snapshot?.filled;
  useEffect(() => {
    if (!active || filled === undefined) return;
    const id = window.setTimeout(() => void reload(), 1000);
    return () => window.clearTimeout(id);
  }, [active, filled, reload]);

  const options: SaveOptions = {
    target: target === "original" ? { kind: "original" } : { kind: "copy", path },
    export: { mark: mark ? MARK : undefined, styles, history },
    after,
  };

  const run = async (label: string, f: () => Promise<void>) => {
    setBusy(label);
    setError(null);
    try {
      await f();
    } catch (e) {
      setError(errorText(e));
    } finally {
      setBusy(null);
    }
  };

  /** 저장: 먼저 원본과 비교하고, 고를 것이 없으면 바로 저장한다. */
  const onSave = () =>
    run("저장하는 중…", async () => {
      setResult(null);
      const c = await api.saveCheck(options);
      const needsReview = c.report.conflicts.length > 0 || c.report.suspicious || c.targetExists;
      if (needsReview) {
        setCheck(c);
        return;
      }
      await doSaveWith(c, {}, false);
    });

  /** 확인 결과(`c`)와 고른 값으로 저장한다. */
  const doSaveWith = async (c: SaveCheck, decisions: Record<string, "collected" | "excel">, forced: boolean) => {
    const pick = (want: "collected" | "excel") =>
      c.report.conflicts.filter((x) => (decisions[key(x.at)] ?? "collected") === want).map((x) => x.at);
    const r = await api.saveRun(options, pick("excel"), pick("collected"), forced);
    setResult(r);
    setCheck(null);
    setChoice({});
    setForce(false);
    if (r.stopped) onStopped();
    await reload();
  };

  const browse = async () => {
    const ext = path.split(".").pop() || "xlsx";
    const picked = await saveDialog({
      defaultPath: path || info?.defaultCopy,
      filters: [{ name: `Excel 통합문서 (.${ext})`, extensions: [ext] }],
    });
    if (picked) setPath(picked);
  };

  const s = info?.summary;
  const pct = s && s.required > 0 ? Math.round((s.filled / s.required) * 100) : 100;
  const since = info?.lastSave && s ? s.historyEntries - info.lastSave.historyEntries : 0;
  const people = useMemo(
    () => (s?.people ?? []).slice().sort((a, b) => b.required - b.filled - (a.required - a.filled)),
    [s],
  );
  const origin = check?.origin ?? { row: 0, col: 0 };
  const addr = (at: CellRef) => a1(origin.row + at.row, origin.col + at.col);
  const conflicts: Conflict[] = check?.report.conflicts ?? [];
  const reviewReady = !check?.report.suspicious || force;

  if (!info) {
    return (
      <div className="screen save-screen">
        <div className="muted">{error ?? "불러오는 중…"}</div>
      </div>
    );
  }

  return (
    <div className="screen save-screen">
      <section className="card save-summary" aria-label="취합 요약">
        <div className="card-head">
          <b className="big">취합 요약</b>
          <span className="muted small">
            {info.workbook} · {info.sheet}
          </span>
          <span className="spacer" />
          {info.closed ? (
            <span className="warn">● 마감됨 (참여자 읽기 전용)</span>
          ) : (
            <span className="ok">● 입력 받는 중{info.running ? "" : " (서비스 중지됨)"}</span>
          )}
        </div>
        {s && (
          <>
            <div className="completion" aria-label="필수 완료율">
              <span className="pct">{pct}%</span>
              <div className="grow">
                <div className="bar">
                  <span style={{ width: `${pct}%` }} />
                </div>
                <div className="small muted">
                  필수 입력 {s.filled.toLocaleString()} / {s.required.toLocaleString()} · 미입력{" "}
                  {(s.required - s.filled).toLocaleString()}칸
                  {s.unassigned > 0 && <span className="warn"> · 배정 안 된 필수 칸 {s.unassigned}</span>}
                </div>
              </div>
            </div>
            <div className="stat-row small">
              <span>
                바뀐 칸 <b>{s.changedCells.toLocaleString()}</b>
              </span>
              <span>
                입력한 사람 <b>{s.changedBy.length}</b>명
              </span>
              <span>
                변경 기록 <b>{s.historyEntries.toLocaleString()}</b>건
              </span>
              <span>마지막 변경 {s.lastChangeMs ? clock(s.lastChangeMs) : "–"}</span>
            </div>
            <div className="missing-table" role="table" aria-label="담당자별 미입력">
              <div className="mrow mhead" role="row">
                <span role="columnheader">담당자</span>
                <span role="columnheader">필수 입력</span>
                <span role="columnheader">미입력 칸</span>
              </div>
              {people.map((p) => {
                const left = p.required - p.filled;
                const ratio = p.required > 0 ? Math.round((p.filled / p.required) * 100) : 100;
                return (
                  <div key={p.name} className="mrow" role="row" aria-label={p.name}>
                    <span className="pname">
                      <span className="mmg-dot" style={{ background: personColor(p.color) }} />
                      {p.name}
                    </span>
                    <span className="pprogress">
                      <span className="bar">
                        <span style={{ width: `${ratio}%` }} />
                      </span>
                      {p.filled.toLocaleString()} / {p.required.toLocaleString()}
                    </span>
                    <span className={`mono small${left > 0 ? "" : " ok"}`}>
                      {left === 0
                        ? "✓ 완료"
                        : `${p.missing.join(", ")}${p.more > 0 ? ` 외 ${p.more.toLocaleString()}칸` : ""}`}
                    </span>
                  </div>
                );
              })}
            </div>
          </>
        )}
      </section>

      <section className="card save-panel" aria-label="엑셀로 저장">
        <div className="card-head">
          <b className="big">엑셀로 저장</b>
        </div>
        <div className="small">
          <span className="muted">원본</span> {info.source?.name ?? info.workbook} · {info.sheet}{" "}
          {info.source ? (
            <span className="muted">(Excel에 열려 있음{info.source.background ? " · 숨은 Excel" : ""})</span>
          ) : (
            <span className="muted">(닫혀 있음 — 저장할 때 다시 엽니다)</span>
          )}
        </div>

        <div className="field">
          저장 위치
          <label className="radio">
            <input type="radio" name="target" checked={target === "copy"} onChange={() => setTarget("copy")} />
            다른 이름으로 저장 (원본은 그대로)
          </label>
          {target === "copy" && (
            <div className="path-row">
              <input aria-label="저장할 파일" className="mono" value={path} onChange={(e) => setPath(e.target.value)} />
              <button className="ghost small" onClick={() => void browse()}>
                찾아보기…
              </button>
            </div>
          )}
          <label className="radio">
            <input
              type="radio"
              name="target"
              checked={target === "original"}
              disabled={!info.sourcePath}
              onChange={() => setTarget("original")}
            />
            원본 통합문서에 반영하고 저장
            {!info.sourcePath && <span className="muted small"> — 저장한 적 없는 새 문서라 쓸 수 없습니다</span>}
          </label>
          {target === "original" && info.source && !info.source.saved && (
            <div className="warn small">원본에 Excel에서 저장하지 않은 변경이 있습니다. 함께 저장됩니다.</div>
          )}
        </div>

        <div className="field">
          함께 넣기
          <label className="check">
            <input type="checkbox" checked={history} onChange={(e) => setHistory(e.target.checked)} />
            ‘변경 이력’ 시트 추가 {s ? `(${s.historyEntries.toLocaleString()}건)` : ""}
          </label>
          <label className="check">
            <input type="checkbox" checked={mark} onChange={(e) => setMark(e.target.checked)} />
            바뀐 칸을 노란색으로 표시
          </label>
          {s && (s.styleCells > 0 || s.mergeChanges > 0) && (
            <label className="check">
              <input type="checkbox" checked={styles} onChange={(e) => setStyles(e.target.checked)} />
              앱에서 바꾼 서식·병합도 반영 (서식 {s.styleCells.toLocaleString()}칸
              {s.mergeChanges > 0 ? ` · 병합 ${s.mergeChanges}` : ""})
            </label>
          )}
          <span className="muted small">
            수식 칸과 원래 서식은 건드리지 않습니다. 수식은 저장할 때 Excel이 다시 계산합니다.
          </span>
        </div>

        <div className="field">
          저장 뒤
          {(
            [
              ["keep", "서비스 계속 (참여자가 계속 입력)"],
              ["readOnly", "읽기 전용으로 전환 (취합 마감)"],
              ["stop", "서비스 종료"],
            ] as const
          ).map(([v, label]) => (
            <label key={v} className="radio">
              <input type="radio" name="after" checked={after === v} onChange={() => setAfter(v)} />
              {label}
            </label>
          ))}
        </div>

        {check && (
          <div className="review" role="region" aria-label="저장 전 확인">
            {conflicts.length > 0 && (
              <>
                <div>
                  <b className="warn">Excel에서도 고친 칸 {conflicts.length}개</b>
                  <span className="muted small"> — 불러온 뒤 원본에서 직접 바뀌었습니다. 어느 값을 쓸지 고르세요.</span>
                </div>
                <div className="row">
                  <button
                    className="ghost small"
                    onClick={() => setChoice(Object.fromEntries(conflicts.map((c) => [key(c.at), "collected"])))}
                  >
                    모두 취합 값
                  </button>
                  <button
                    className="ghost small"
                    onClick={() => setChoice(Object.fromEntries(conflicts.map((c) => [key(c.at), "excel"])))}
                  >
                    모두 Excel 값
                  </button>
                </div>
                <div className="conflict-table" role="table" aria-label="충돌 칸">
                  <div className="crow chead" role="row">
                    <span>셀</span>
                    <span>항목</span>
                    <span>불러올 때</span>
                    <span>지금 Excel</span>
                    <span>취합 값</span>
                  </div>
                  {conflicts.map((c) => {
                    const k = key(c.at);
                    const pick = choice[k] ?? "collected";
                    return (
                      <div key={k} className="crow" role="row" aria-label={addr(c.at)}>
                        <span className="mono">{addr(c.at)}</span>
                        <span>{check.labels[c.at.col] ?? ""}</span>
                        <span className="muted">{show(c.original)}</span>
                        <label className={`pick${pick === "excel" ? " on" : ""}`}>
                          <input
                            type="radio"
                            name={`c${k}`}
                            checked={pick === "excel"}
                            onChange={() => setChoice({ ...choice, [k]: "excel" })}
                          />
                          {show(c.excel)}
                        </label>
                        <label className={`pick${pick === "collected" ? " on" : ""}`}>
                          <input
                            type="radio"
                            name={`c${k}`}
                            checked={pick === "collected"}
                            onChange={() => setChoice({ ...choice, [k]: "collected" })}
                          />
                          {show(c.collected)}
                        </label>
                      </div>
                    );
                  })}
                </div>
              </>
            )}
            {check.report.driftCount > 0 && (
              <div className={`small${check.report.suspicious ? " error" : " muted"}`}>
                {check.report.suspicious
                  ? `원본 표 위치가 바뀐 것 같습니다: 취합하지 않는 칸 ${check.report.driftCount.toLocaleString()}개가 불러올 때와 다릅니다(행·열을 넣거나 지웠을 수 있음). 그대로 저장하면 엉뚱한 칸에 쓸 수 있습니다.`
                  : `참고: 취합하지 않는 칸 ${check.report.driftCount}개가 Excel에서 바뀌었습니다(${check.report.drift
                      .slice(0, 5)
                      .map(addr)
                      .join(", ")}). 이 칸들은 건드리지 않습니다.`}
              </div>
            )}
            {check.report.suspicious && (
              <label className="check error">
                <input type="checkbox" checked={force} onChange={(e) => setForce(e.target.checked)} />
                확인했습니다. 그래도 저장
              </label>
            )}
            {check.targetExists && target === "copy" && (
              <div className="warn small">같은 이름의 파일이 있습니다. 저장하면 덮어씁니다.</div>
            )}
            <div className="row">
              <button
                disabled={busy !== null || !reviewReady}
                onClick={() => check && void run("저장하는 중…", () => doSaveWith(check, choice, force))}
              >
                이대로 저장
              </button>
              <button className="ghost" onClick={() => setCheck(null)}>
                취소
              </button>
            </div>
          </div>
        )}

        {error && <div className="error small">{error}</div>}
        {result && (
          <div className="save-result" role="status">
            <b className={result.report.mismatched.length === 0 ? "ok" : "warn"}>
              {result.report.mismatched.length === 0 ? "✓ 저장했습니다" : "저장했지만 확인이 필요합니다"}
            </b>
            <code className="mono small">{result.report.path}</code>
            <span className="small">
              값 {result.report.written.toLocaleString()}칸
              {result.report.styled > 0 ? ` · 서식 ${result.report.styled.toLocaleString()}칸` : ""}
              {result.report.merges > 0 ? ` · 병합 ${result.report.merges}` : ""}
              {result.report.historySheet ? ` · ‘${result.report.historySheet}’ 시트` : ""}
              {result.kept > 0 ? ` · Excel 값 유지 ${result.kept}칸` : ""} ·{" "}
              {result.report.mismatched.length === 0
                ? "다시 읽어 확인함"
                : `다시 읽었더니 다른 칸 ${result.report.mismatched.length}개`}{" "}
              · {(Number(result.report.ms) / 1000).toFixed(1)}초
            </span>
            <span className="small">
              {result.closed ? "취합을 마감했습니다(참여자 읽기 전용). " : ""}
              {result.stopped ? "서비스를 멈췄습니다. " : ""}
            </span>
            <button className="ghost small" onClick={() => void api.revealFile(result.report.path)}>
              탐색기에서 보기
            </button>
          </div>
        )}
        {info.lastSave && !result && (
          <div className="small muted">
            마지막 저장 {clock(info.lastSave.atMs)} · {info.lastSave.original ? "원본에 반영" : "다른 이름으로"}
            {since > 0 ? ` · 그 뒤 변경 ${since}건` : " · 그 뒤 변경 없음"}
          </div>
        )}

        <span className="spacer" />
        <div className="row save-actions">
          {info.closed ? (
            <button
              className="ghost"
              disabled={busy !== null}
              onClick={() => void run("", () => api.collectClose(false).then(reload))}
            >
              취합 다시 열기
            </button>
          ) : (
            <button
              className="ghost"
              disabled={busy !== null}
              onClick={() => void run("", () => api.collectClose(true).then(reload))}
            >
              저장 없이 취합 마감
            </button>
          )}
          <span className="spacer" />
          <button
            disabled={busy !== null || check !== null || (target === "copy" && !path.trim())}
            onClick={() => void onSave()}
          >
            {busy ?? "엑셀로 저장"}
          </button>
        </div>
      </section>
    </div>
  );
}
