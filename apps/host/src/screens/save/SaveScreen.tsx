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
  SheetCell,
} from "@mm/shared";
import { a1, personColor, sheetPrefix } from "@mm/shared";
import { save as saveDialog } from "@tauri-apps/plugin-dialog";
import { useCallback, useEffect, useMemo, useState } from "react";

import { errorText } from "../../api";
import { useApi } from "../../job";

interface Props {
  active: boolean;
  snapshot: HostSnapshot | null;
  /** 서비스를 멈췄을 때(저장 뒤 '서비스 종료') 바깥 상태도 맞춘다. */
  onStopped: () => void;
  /** '새 취합 시작': 새 업무를 만들어 그 업무의 1 불러오기로(이 업무는 그대로). */
  onRestart: () => void;
  /** '업무 끝내기'(확인 대화상자). */
  onEndJob: () => void;
}

/** 바뀐 칸 표시 색(연한 노랑). */
const MARK = 0xfff3b0;

/** 저장 중 단추 글자. */
const SAVING = "저장하는 중…";

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

/** 시트 안의 충돌 칸. */
interface SheetConflict {
  sheet: number;
  c: Conflict;
}

const key = (sheet: number, at: CellRef) => `${sheet}:${at.row}:${at.col}`;

/** 저장 전 확인의 칸 주소: 시트가 여럿이면 `인력계획!F7`. */
function checkAddr(check: SaveCheck | null, sheet: number, at: CellRef): string {
  const sc = check?.sheets[sheet];
  const o = sc?.origin ?? { row: 0, col: 0 };
  const cell = a1(o.row + at.row, o.col + at.col);
  return sc && check!.sheets.length > 1 ? `${sheetPrefix(sc.name)}${cell}` : cell;
}

/** 6 저장·마감: 취합 요약 → 원본과 비교 → 엑셀로 저장 → (선택) 마감. */
export function SaveScreen({ active, snapshot, onStopped, onRestart, onEndJob }: Props) {
  const api = useApi();
  const [info, setInfo] = useState<SaveInfo | null>(null);
  const [target, setTarget] = useState<"original" | "copy">("copy");
  const [path, setPath] = useState("");
  const [history, setHistory] = useState(true);
  const [mark, setMark] = useState(false);
  const [styles, setStyles] = useState(true);
  /** 취소 표시한 행에 취소선 + 회색 채우기를 쓴다. */
  const [cancelled, setCancelled] = useState(true);
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
    export: { mark: mark ? MARK : undefined, styles, history, cancelled },
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
    run(SAVING, async () => {
      setResult(null);
      const c = await api.saveCheck(options);
      const needsReview = c.sheets.some((x) => x.report.conflicts.length > 0) || c.suspicious || c.targetExists;
      if (needsReview) {
        setCheck(c);
        return;
      }
      await doSaveWith(c, {}, false);
    });

  /** 확인 결과(`c`)와 고른 값으로 저장한다. */
  const doSaveWith = async (c: SaveCheck, decisions: Record<string, "collected" | "excel">, forced: boolean) => {
    const pick = (want: "collected" | "excel"): SheetCell[] =>
      c.sheets.flatMap((sc, sheet) =>
        sc.report.conflicts
          .filter((x) => (decisions[key(sheet, x.at)] ?? "collected") === want)
          .map((x) => ({ sheet, row: x.at.row, col: x.at.col })),
      );
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
  const addr = (sheet: number, at: CellRef) => checkAddr(check, sheet, at);
  const conflicts: SheetConflict[] = (check?.sheets ?? []).flatMap((sc, sheet) =>
    sc.report.conflicts.map((c) => ({ sheet, c })),
  );
  const drifted = (check?.sheets ?? []).map((sc, sheet) => ({ sc, sheet })).filter((x) => x.sc.report.driftCount > 0);
  const reviewReady = !check?.suspicious || force;
  const perSheet = s && s.sheets.length > 1 ? s.sheets : [];

  if (!info) {
    return (
      <div className="screen save-screen">
        <div className="muted">{error ?? "불러오는 중…"}</div>
      </div>
    );
  }

  if (result) {
    const ok = result.report.mismatched.length === 0;
    return (
      <div className="screen save-screen">
        <div className="save-result" role="status">
          <div className={`done-mark${ok ? "" : " warn"}`}>{ok ? "✓" : "!"}</div>
          <div className="done-title">{ok ? "저장했어요" : "저장했지만 확인이 필요해요"}</div>
          <code className="done-path">{result.report.path}</code>
          <span className="small muted">
            값 {result.report.written.toLocaleString()}칸
            {result.report.styled > 0 ? ` · 서식 ${result.report.styled.toLocaleString()}칸` : ""}
            {result.report.merges > 0 ? ` · 병합 ${result.report.merges}` : ""}
            {result.report.resized > 0 ? ` · 크기 ${result.report.resized}줄` : ""}
            {result.report.inserted > 0 ? ` · 추가한 행 ${result.report.inserted}개` : ""}
            {result.report.insertedCols > 0 ? ` · 추가한 열 ${result.report.insertedCols}개` : ""}
            {result.report.historySheet ? ` · ‘${result.report.historySheet}’ 시트` : ""}
            {result.kept > 0 ? ` · Excel 값 유지 ${result.kept}칸` : ""} ·{" "}
            {ok ? "다시 읽어 확인함" : `다시 읽었더니 다른 칸 ${result.report.mismatched.length}개`} ·{" "}
            {(Number(result.report.ms) / 1000).toFixed(1)}초
          </span>
          {(result.closed || result.stopped) && (
            <span className="small">
              {result.closed ? "취합을 마감했습니다(참여자 읽기 전용). " : ""}
              {result.stopped ? "이 업무의 공유를 멈췄습니다. " : ""}
            </span>
          )}
          <div className="row done-actions">
            <button className="ghost" onClick={() => void api.revealFile(result.report.path)}>
              폴더 열기
            </button>
            <button className="ghost" onClick={() => setResult(null)}>
              저장 화면으로 돌아가기
            </button>
            <button className="ghost" onClick={onEndJob}>
              업무 끝내기
            </button>
            <button onClick={onRestart}>새 취합 시작</button>
          </div>
        </div>
      </div>
    );
  }

  return (
    <div className="screen save-screen">
      <div className="save-body">
        {s && (
          <>
            <div className="tiles" aria-label="취합 요약">
              <div className="tile">
                <div className="tile-v">{s.changedCells.toLocaleString()}</div>
                <div className="tile-l">바뀐 칸</div>
              </div>
              <div className="tile">
                <div className="tile-v">{s.changedBy.length}</div>
                <div className="tile-l">입력한 참여자</div>
              </div>
              <div className="tile" aria-label="필수 완료율">
                <div className={`tile-v ${s.required - s.filled > 0 ? "warn" : "ok"}`}>
                  {(s.required - s.filled).toLocaleString()}
                  <span className="tile-sub">완료 {pct}%</span>
                </div>
                <div className="tile-l">
                  {s.required - s.filled > 0 ? "필수 칸이 비어 있어요 (그대로 저장 가능)" : "필수 칸 모두 채움"}
                </div>
              </div>
            </div>
            <div className="stat-row small muted">
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
              {s.addedRows > 0 && (
                <span title="앱에서 표 아래에 추가한 행. 저장하면 Excel 표 아래에 끼워 넣습니다">
                  추가된 행 <b>{s.addedRows.toLocaleString()}</b>
                </span>
              )}
              {s.addedCols > 0 && (
                <span title="앱에서 표 오른쪽에 추가한 열. 저장하면 Excel 표 오른쪽에 끼워 넣습니다">
                  추가된 열 <b>{s.addedCols.toLocaleString()}</b>
                </span>
              )}
              {s.cancelledRows > 0 && (
                <span title="취소 표시한 행(필수 칸으로 세지 않습니다)">
                  취소 행 <b>{s.cancelledRows.toLocaleString()}</b>
                </span>
              )}
              {s.unassigned > 0 && <span>담당자 없는 필수 칸 {s.unassigned}(누구나 입력)</span>}
              <span className="spacer" />
              {info.closed ? (
                <span className="warn">● 마감됨 (참여자 읽기 전용)</span>
              ) : (
                <span className="ok">● 입력 받는 중{info.running ? "" : " (서비스 중지됨)"}</span>
              )}
            </div>
            {perSheet.length > 0 && (
              <div className="sheet-summary small" role="table" aria-label="시트별 요약">
                {perSheet.map((x) => (
                  <div key={x.name} className="stat-row" role="row" aria-label={x.name}>
                    <b>{x.name}</b>
                    <span>
                      바뀐 칸 <b>{x.changedCells.toLocaleString()}</b>
                    </span>
                    <span className={x.required > x.filled ? "warn" : "ok"}>
                      {x.required > x.filled
                        ? `필수 미입력 ${(x.required - x.filled).toLocaleString()}`
                        : "필수 칸 모두 채움"}
                    </span>
                    {x.addedRows > 0 && <span>추가된 행 {x.addedRows.toLocaleString()}</span>}
                    {x.addedCols > 0 && <span>추가된 열 {x.addedCols.toLocaleString()}</span>}
                    {x.cancelledRows > 0 && <span>취소 행 {x.cancelledRows.toLocaleString()}</span>}
                    {x.styleCells > 0 && <span className="muted">서식 {x.styleCells.toLocaleString()}칸</span>}
                  </div>
                ))}
              </div>
            )}
          </>
        )}

        {check && (
          <div className="review" role="region" aria-label="저장 전 확인">
            {conflicts.length > 0 && (
              <>
                <div>
                  <div className="card-title">공유하는 동안 원본 파일이 바뀌었어요</div>
                  <div className="muted-2">같은 칸 {conflicts.length}개가 겹칩니다. 남길 값을 눌러 고르세요.</div>
                </div>
                <div className="row">
                  <button
                    className="ghost small"
                    onClick={() =>
                      setChoice(Object.fromEntries(conflicts.map((x) => [key(x.sheet, x.c.at), "collected"])))
                    }
                  >
                    모두 참여자 값
                  </button>
                  <button
                    className="ghost small"
                    onClick={() => setChoice(Object.fromEntries(conflicts.map((x) => [key(x.sheet, x.c.at), "excel"])))}
                  >
                    모두 원본 값
                  </button>
                </div>
                <div className="conflicts" role="table" aria-label="충돌 칸">
                  {conflicts.map(({ sheet, c }) => {
                    const k = key(sheet, c.at);
                    const pick = choice[k] ?? "collected";
                    const opt = (want: "excel" | "collected", label: string, v: CellValue | undefined) => (
                      <label className={`opt${pick === want ? " on" : ""}`}>
                        <input
                          type="radio"
                          name={`c${k}`}
                          checked={pick === want}
                          onChange={() => setChoice({ ...choice, [k]: want })}
                        />
                        <span className="ring" />
                        <span className="opt-l">{label}</span>
                        <span className="opt-v">{show(v)}</span>
                      </label>
                    );
                    return (
                      <div key={k} className="conflict" role="row" aria-label={addr(sheet, c.at)}>
                        <div>
                          <div className="where">{check.sheets[sheet]?.labels[c.at.col] || "칸"}</div>
                          <div className="mono muted small">
                            {addr(sheet, c.at)} · 불러올 때 {show(c.original)}
                          </div>
                        </div>
                        {opt("excel", "원본", c.excel)}
                        {opt("collected", "참여자", c.collected)}
                      </div>
                    );
                  })}
                </div>
              </>
            )}
            {drifted.map(({ sc, sheet }) => {
              const where = check.sheets.length > 1 ? `‘${sc.name}’ 시트의 ` : "";
              return (
                <div key={sheet} className={`small${sc.report.suspicious ? " error" : " muted"}`}>
                  {sc.report.suspicious
                    ? `원본 표 위치가 바뀐 것 같습니다: ${where}취합하지 않는 칸 ${sc.report.driftCount.toLocaleString()}개가 불러올 때와 다릅니다(행·열을 넣거나 지웠을 수 있음). 그대로 저장하면 엉뚱한 칸에 쓸 수 있습니다.`
                    : `참고: ${where}취합하지 않는 칸 ${sc.report.driftCount}개가 Excel에서 바뀌었습니다(${sc.report.drift
                        .slice(0, 5)
                        .map((at) => addr(sheet, at))
                        .join(", ")}). 이 칸들은 건드리지 않습니다.`}
                </div>
              );
            })}
            {check.suspicious && (
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
                onClick={() => check && void run(SAVING, () => doSaveWith(check, choice, force))}
              >
                이대로 저장
              </button>
              <button className="ghost" onClick={() => setCheck(null)}>
                취소
              </button>
            </div>
          </div>
        )}

        <section className="save-how" aria-label="엑셀로 저장">
          <div className="card-head">
            <b>저장 방법</b>
            <span className="muted small">
              원본 {info.source?.name ?? info.workbook} · {info.sheets.join(", ")}{" "}
              {info.source
                ? `(Excel에 열려 있음${info.source.background ? " · 숨은 Excel" : ""})`
                : "(닫혀 있음 — 저장할 때 다시 엽니다)"}
            </span>
          </div>
          <div className="choice-cards">
            <label className={`choice${target === "copy" ? " on" : ""}`}>
              <input type="radio" name="target" checked={target === "copy"} onChange={() => setTarget("copy")} />
              <span className="ring" />
              <span>
                <b>새 파일로 저장</b>
                <span className="choice-desc">
                  원본은 그대로 두고 모은 내용을 새 파일로 만들어요. 처음이라면 이쪽을 권해요.
                </span>
              </span>
            </label>
            <label className={`choice${target === "original" ? " on" : ""}${info.sourcePath ? "" : " off"}`}>
              <input
                type="radio"
                name="target"
                checked={target === "original"}
                disabled={!info.sourcePath}
                onChange={() => setTarget("original")}
              />
              <span className="ring" />
              <span>
                <b>원본 파일에 반영</b>
                <span className="choice-desc">
                  {info.sourcePath
                    ? "원본 통합문서에 바로 덮어써요. 되돌리려면 스냅샷을 이용하세요."
                    : "저장한 적 없는 새 문서라 쓸 수 없습니다."}
                </span>
              </span>
            </label>
          </div>
          {target === "copy" && (
            <label className="path-row">
              <span className="muted">파일 이름</span>
              <input aria-label="저장할 파일" className="mono" value={path} onChange={(e) => setPath(e.target.value)} />
              <button className="ghost" onClick={() => void browse()}>
                찾아보기…
              </button>
            </label>
          )}
          {target === "original" && info.source && !info.source.saved && (
            <div className="warn small">원본에 Excel에서 저장하지 않은 변경이 있습니다. 함께 저장됩니다.</div>
          )}
        </section>

        <details className="more" open>
          <summary>저장 옵션</summary>
          <div className="more-body save-options">
            <div className="field">
              <span className="section-label">함께 넣기</span>
              <label className="check">
                <input type="checkbox" checked={history} onChange={(e) => setHistory(e.target.checked)} />
                ‘변경 이력’ 시트 추가 {s ? `(${s.historyEntries.toLocaleString()}건)` : ""}
              </label>
              <label className="check">
                <input type="checkbox" checked={mark} onChange={(e) => setMark(e.target.checked)} />
                바뀐 칸을 노란색으로 표시
              </label>
              {s && (s.styleCells > 0 || s.mergeChanges > 0 || s.sizeChanges > 0) && (
                <label className="check">
                  <input type="checkbox" checked={styles} onChange={(e) => setStyles(e.target.checked)} />
                  앱에서 바꾼 서식·병합·크기도 반영 (
                  {[
                    s.styleCells > 0 ? `서식 ${s.styleCells.toLocaleString()}칸` : "",
                    s.mergeChanges > 0 ? `병합 ${s.mergeChanges}` : "",
                    s.sizeChanges > 0 ? `열 너비·행 높이 ${s.sizeChanges}줄` : "",
                  ]
                    .filter(Boolean)
                    .join(" · ")}
                  )
                </label>
              )}
              {s && (s.cancelledRows > 0 || s.cancelChanges > 0) && (
                <label className="check">
                  <input type="checkbox" checked={cancelled} onChange={(e) => setCancelled(e.target.checked)} />
                  취소 표시한 행: 취소선 + 회색 채우기 ({s.cancelledRows.toLocaleString()}행
                  {s.cancelChanges > s.cancelledRows ? ` · 표시를 푼 행은 원래 서식으로` : ""})
                </label>
              )}
              <span className="muted small">
                수식 칸과 원래 서식은 건드리지 않습니다. 수식은 저장할 때 Excel이 다시 계산합니다.
              </span>
            </div>
            <div className="field">
              <span className="section-label">저장 뒤</span>
              {(
                [
                  ["keep", "서비스 계속 (참여자가 계속 입력)"],
                  ["readOnly", "읽기 전용으로 전환 (취합 마감)"],
                  ["stop", "이 업무 공유 중지 (다른 업무는 계속)"],
                ] as const
              ).map(([v, label]) => (
                <label key={v} className="check">
                  <input type="radio" name="after" checked={after === v} onChange={() => setAfter(v)} />
                  {label}
                </label>
              ))}
            </div>
          </div>
        </details>

        {error && <div className="error small">{error}</div>}
        {info.lastSave && (
          <div className="small muted">
            마지막 저장 {clock(info.lastSave.atMs)} · {info.lastSave.original ? "원본에 반영" : "다른 이름으로"}
            {since > 0 ? ` · 그 뒤 변경 ${since}건` : " · 그 뒤 변경 없음"}
          </div>
        )}

        <div className="row save-actions">
          <button
            className="save-btn"
            disabled={busy !== null || check !== null || (target === "copy" && !path.trim())}
            onClick={() => void onSave()}
          >
            {busy === SAVING ? busy : "저장하기"}
          </button>
          <button
            className="ghost"
            disabled={busy !== null}
            onClick={onEndJob}
            title="저장하지 않고 이 업무를 끝내고 목록에서 뺍니다(저장하지 않은 변경이 있으면 한 번 더 묻습니다)"
          >
            저장 없이 업무 끝내기
          </button>
          <span className="spacer" />
          {info.closed && (
            // 저장 뒤 '읽기 전용으로 전환'으로 마감한 업무를 다시 여는 길.
            <button
              className="ghost"
              disabled={busy !== null}
              onClick={() => void run("다시 여는 중…", () => api.collectClose(false).then(reload))}
            >
              {busy === "다시 여는 중…" ? busy : "취합 다시 열기"}
            </button>
          )}
        </div>

        {s && people.length > 0 && (
          <details className="more">
            <summary>
              담당자별 미입력{" "}
              <span className="muted small">{people.filter((p) => p.required > p.filled).length}명</span>
            </summary>
            <div className="more-body">
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
            </div>
          </details>
        )}
      </div>
    </div>
  );
}
