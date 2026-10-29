import type { GridSelection } from "@mm/grid";
import { SheetGrid } from "@mm/grid";
import type { ImportItems, JobSummary, ReadStage, SheetModel } from "@mm/shared";
import { colLetters, colSpanLabel, formatRange, parseRange } from "@mm/shared";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { useEffect, useState } from "react";

import { useStepFooter } from "../../frame";
import { useJobId } from "../../job";
import { SheetTabs } from "../../SheetTabs";
import { PreviewModal } from "./PreviewModal";
import type { ImportState } from "./useImport";
import { EXCEL_EXTENSIONS, useImport } from "./useImport";

const STAGE_LABEL: Record<ReadStage, string> = {
  values: "값",
  formulas: "수식",
  merges: "병합",
  styles: "서식",
  dimensions: "열 너비·행 높이",
  validations: "데이터 유효성",
  done: "마무리",
};

const ITEMS: { key: keyof ImportItems; label: string }[] = [
  { key: "formulas", label: "수식 (참여자에겐 결과만)" },
  { key: "merges", label: "셀 병합" },
  { key: "fonts", label: "글꼴 · 글자색" },
  { key: "borders", label: "테두리" },
  { key: "fills", label: "배경색" },
  { key: "dimensions", label: "열 너비 · 행 높이" },
  { key: "validations", label: "데이터 유효성 → 입력 형식" },
];

/** 틀 고정 열 선택지는 앞쪽 몇 열까지만 둔다(참여자 화면이 좁아지지 않게). */
const MAX_FROZEN_COLS = 8;

interface Props {
  active: boolean;
  /** 이미 확정한 시트가 있다(바꾼 게 없으면 '다음'은 확정 없이 넘어간다). */
  committed: boolean;
  /** 모든 업무(같은 통합문서·시트를 취합 중인 다른 업무를 알린다). */
  jobs: JobSummary[];
  onExcelStatus: (running: boolean) => void;
  onCommitted: (m: SheetModel[]) => void;
  onNext: () => void;
}

/** 1a 엑셀 불러오기. */
export function ImportScreen({ active, committed, jobs, onExcelStatus, onCommitted, onNext }: Props) {
  const s = useImport(onExcelStatus, onCommitted);
  const job = useJobId();
  // 같은 시트를 다른 업무가 취합 중이면 알린다(막지는 않는다 — 원본에 반영할 때 서로 덮어쓸 수 있다).
  const bookId = s.book?.id.toLowerCase();
  const overlap = jobs.filter(
    (j) =>
      j.id !== job && bookId && j.workbookId.toLowerCase() === bookId && j.sheets.some((x) => s.selected.includes(x)),
  );
  const [didInit, setDidInit] = useState(false);
  useEffect(() => {
    if (didInit) return;
    setDidInit(true);
    void s.refresh();
  }, [didInit, s.refresh]);

  // 확정한 시트가 있고 새로 고른 게 없으면(이어서 하기 포함) 다시 확정하지 않고 넘어간다.
  const reuse = committed && (!s.book || !s.changedSinceCommit);
  useStepFooter(active, {
    onNext: reuse ? onNext : () => void s.commit(),
    nextDisabled: !reuse && (!s.book || s.selected.length === 0 || !s.result || s.stale || s.busy !== null),
    hint: s.book || committed ? undefined : "파일을 고르면 다음 단계로 넘어갈 수 있어요",
  });

  return (
    <div className="screen import-screen">
      {s.book ? (
        <div className="import-grid">
          <div className="col">
            <FileCard s={s} />
            {overlap.length > 0 && (
              <div className="warn small" role="note">
                같은 시트를 다른 업무({overlap.map((j) => `‘${j.title || "새 취합"}’`).join(", ")})에서도 취합 중이에요.
                두 업무 모두 원본에 반영하면 서로의 값을 덮어쓸 수 있으니, 한쪽은 다른 이름으로 저장하세요.
              </div>
            )}
            <SheetCard s={s} />
            <RangeCard s={s} />
            <ItemsCard s={s} />
            <div className="info-note">
              서식·수식·병합 셀은 원본 그대로 가져옵니다. 원본 파일은 마지막 저장 단계 전까지 바뀌지 않아요.
            </div>
          </div>
          <div className="col wide">
            <PreviewCard s={s} />
            <ImportStatus s={s} />
          </div>
        </div>
      ) : (
        <Picker s={s} active={active} />
      )}
    </div>
  );
}

/** 끌어다 놓기 · 파일 선택 · 실행 중인 Excel의 통합문서. */
function Picker({ s, active }: { s: ImportState; active: boolean }) {
  const [over, setOver] = useState(false);
  const openFile = s.openFile;

  // Tauri 창에 파일을 끌어다 놓으면 경로가 온다(브라우저 drop 이벤트로는 경로를 알 수 없다).
  useEffect(() => {
    if (!active) return;
    let off: (() => void) | undefined;
    let alive = true;
    try {
      void getCurrentWebview()
        .onDragDropEvent((e) => {
          const p = e.payload;
          if (p.type === "enter" || p.type === "over") setOver(true);
          else if (p.type === "leave") setOver(false);
          else if (p.type === "drop") {
            setOver(false);
            const path = p.paths.find((x) => EXCEL_EXTENSIONS.includes(x.split(".").pop()?.toLowerCase() ?? ""));
            if (path) void openFile(path);
          }
        })
        .then((f) => (alive ? (off = f) : f()))
        .catch(() => {});
    } catch {
      // Tauri 밖(브라우저 개발 서버)에서는 끌어다 놓기를 쓰지 않는다.
    }
    return () => {
      alive = false;
      off?.();
    };
  }, [active, openFile]);

  const books = s.status?.workbooks ?? [];
  return (
    <div className="picker">
      <div className={`dropzone${over ? " over" : ""}`}>
        <div className="drop-title">엑셀 파일을 여기로 끌어다 놓으세요</div>
        <div className="muted">.xlsx 또는 .xls 파일</div>
        <button onClick={() => void s.openFile()} disabled={s.busy !== null}>
          {s.busy === "open" ? "여는 중…" : "파일 선택"}
        </button>
      </div>
      <section className="picker-list" aria-label="통합문서">
        <div className="card-head">
          <span className="section-label">실행 중인 Excel</span>
          <span className="spacer" />
          <button className="ghost small" onClick={() => void s.refresh()} disabled={s.busy !== null}>
            ↻ 다시 찾기
          </button>
        </div>
        {s.busy === "list" && <div className="muted small">Excel을 찾는 중…</div>}
        {s.status && books.length === 0 && (
          <div className="info-note small">
            {s.status.running
              ? "실행 중인 Excel에 열린 통합문서가 없습니다."
              : "실행 중인 Excel이 없습니다. ‘파일 선택’으로 불러오면 보이지 않는 Excel에서 읽습니다."}
          </div>
        )}
        {books.length > 0 && (
          <div className="list">
            {books.map((b) => (
              <button key={b.id} className="list-item book" onClick={() => s.pick(b)}>
                <span>{b.name}</span>
                <span className="muted">
                  {b.path || "저장 안 됨"} · 시트 {b.sheets.length}개{b.background ? " · 백그라운드" : ""}
                </span>
              </button>
            ))}
          </div>
        )}
        {s.error && <div className="error small">{s.error}</div>}
      </section>
    </div>
  );
}

function FileCard({ s }: { s: ImportState }) {
  const b = s.book!;
  const ext = (b.name.split(".").pop() || "xlsx").toUpperCase().slice(0, 4);
  return (
    <section className="file-card" aria-label="통합문서">
      <span className="file-icon">{ext}</span>
      <div className="file-meta">
        <div className="file-name">{b.name}</div>
        <div className="muted">
          {b.path || "저장 안 됨"} · 시트 {b.sheets.length}개{b.background ? " · 백그라운드" : ""}
        </div>
      </div>
      <button className="ghost" onClick={s.unpick} disabled={s.busy === "commit"}>
        다른 파일
      </button>
    </section>
  );
}

/** 함께 채울 시트(여러 개 고를 수 있다): 체크하면 불러오고, 이름을 누르면 그 시트를 미리 보며 범위를 고친다. */
function SheetCard({ s }: { s: ImportState }) {
  const lists = (s.inspection?.validations ?? []).filter((v) => v.rule.kind === "list");
  const many = s.selected.length > 1;
  return (
    <section className="sheet-pick" aria-label="시트">
      <div className="card-head">
        <b>함께 채울 시트</b>
        <span className="muted small">
          {many ? `${s.selected.length}개 시트를 함께 취합합니다` : "여러 개 고를 수 있어요"}
        </span>
      </div>
      <div className="list">
        {s.book!.sheets.map((name) => {
          const picked = s.selected.includes(name);
          const on = name === s.sheet;
          const ins = s.per[name]?.inspection;
          return (
            <div key={name} className={`list-item sheet${on ? " on" : ""}${picked ? " picked" : ""}`}>
              <input
                type="checkbox"
                aria-label={`${name} 불러오기`}
                checked={picked}
                disabled={s.busy === "commit" || (picked && s.selected.length <= 1)}
                onChange={() => s.toggle(name)}
              />
              <button
                className="sheet-name grow"
                aria-pressed={on}
                onClick={() => s.setSheet(name)}
                disabled={s.busy === "inspect" || s.busy === "commit"}
                title={picked ? "이 시트를 미리 보고 범위를 고칩니다" : "이 시트도 함께 불러옵니다"}
              >
                {name}
              </button>
              <span className="muted">
                {ins
                  ? formatRange(ins.table)
                  : on && s.busy === "inspect"
                    ? "감지 중…"
                    : picked
                      ? "확정할 때 읽음"
                      : ""}
              </span>
            </div>
          );
        })}
      </div>
      {lists.map((v) => (
        <div key={formatRange(v.range)} className="muted small">
          {v.rule.kind === "list" && v.rule.source.replace(/^=/, "").replace(/\$/g, "")} → ‘
          {v.label || formatRange(v.range)}’ 목록 원본으로 연결됨
        </div>
      ))}
    </section>
  );
}

function RangeCard({ s }: { s: ImportState }) {
  const { form, inspection } = s;
  const range = form ? parseRange(form.range) : null;
  const auto = inspection && form && range && formatRange(range) === formatRange(inspection.table);
  const start = range ? range.row + 1 : 1;
  const frozenMax = range ? Math.min(range.cols, MAX_FROZEN_COLS) : 0;

  const setHeaderStart = (row: number) => {
    if (!range || !form || row < 1) return;
    const end = range.row + range.rows; // 마지막 행(1부터)
    if (row > end) return;
    s.update({ range: formatRange({ ...range, row: row - 1, rows: end - row + 1 }) });
  };

  return (
    <section className="card range-card" aria-label="범위 · 머리글">
      <div className="card-head">
        <b>범위 · 머리글</b>
        <span className="muted small">범위는 자동으로 찾았어요. 머리글 행은 직접 정하세요.</span>
        {s.busy === "inspect" && <span className="muted small">감지 중…</span>}
      </div>
      {form ? (
        <div className="range-fields">
          <label className="field">
            데이터 범위
            <input
              className={`mono${s.rangeValid ? "" : " invalid"}`}
              value={form.range}
              onChange={(e) => s.update({ range: e.target.value.toUpperCase() })}
              aria-invalid={!s.rangeValid}
            />
          </label>
          <span className={`hint ${auto ? "accent" : "muted"}`}>
            {!s.rangeValid ? "예: A1:I1002" : auto ? "자동 감지됨" : `감지값 ${formatRange(inspection!.table)}`}
          </span>
          <div className="field">
            머리글 행
            <span className="pair">
              <input
                type="number"
                min={1}
                aria-label="머리글 시작 행"
                value={start}
                onChange={(e) => setHeaderStart(Number(e.target.value))}
              />
              ~{/* 머리글이 없으면(기본) 끝 행을 비워 '없음'으로 보인다. 비우면 다시 없음. */}
              <input
                type="number"
                min={start}
                aria-label="머리글 끝 행"
                placeholder="없음"
                value={form.headerRows > 0 ? start + form.headerRows - 1 : ""}
                onChange={(e) =>
                  s.update({
                    headerRows: e.target.value === "" ? 0 : Math.max(0, Number(e.target.value) - start + 1),
                  })
                }
              />
            </span>
          </div>
          <label className="field">
            틀 고정
            <select
              value={Math.min(form.frozenCols, frozenMax)}
              onChange={(e) => s.update({ frozenCols: Number(e.target.value) })}
            >
              {Array.from({ length: frozenMax + 1 }, (_, n) => (
                <option key={n} value={n}>
                  {colSpanLabel(range?.col ?? 0, n)}
                </option>
              ))}
            </select>
          </label>
        </div>
      ) : (
        <div className="muted small">시트를 고르면 표 범위와 머리글을 자동으로 찾습니다.</div>
      )}
      {form && form.headerRows === 0 && (
        <div className="warn small">머리글 행이 없으면 열 이름이 A, B, C…로 보입니다.</div>
      )}
    </section>
  );
}

function ItemsCard({ s }: { s: ImportState }) {
  const items = s.form?.items;
  const off = items ? ITEMS.filter(({ key }) => !items[key]).length : 0;
  return (
    <details className="more" aria-label="가져올 항목">
      <summary>
        가져올 항목
        <span className="muted small">{off === 0 ? "값 · 서식 · 수식 모두" : `${off}개 항목 뺌`}</span>
      </summary>
      <div className="more-body items">
        <label className="check">
          <input type="checkbox" checked disabled /> 값
        </label>
        {ITEMS.map(({ key, label }) => (
          <label key={key} className="check">
            <input
              type="checkbox"
              checked={items ? items[key] : false}
              disabled={!items}
              onChange={(e) => items && s.update({ items: { ...items, [key]: e.target.checked } })}
            />{" "}
            {label}
          </label>
        ))}
      </div>
    </details>
  );
}

function columnsLabel(cols: number[]): string {
  if (cols.length === 0) return "";
  const letters = cols.slice(0, 3).map(colLetters).join("·");
  return ` (${letters}${cols.length > 3 ? " 외" : ""}열)`;
}

/** 고른 시트가 여럿이면 미리보기 아래의 시트 탭(미리 보는 시트를 바꾼다). */
function PreviewTabs({ s }: { s: ImportState }) {
  return (
    <SheetTabs
      names={s.selected}
      active={Math.max(0, s.selected.indexOf(s.sheet ?? ""))}
      onSelect={(i) => s.setSheet(s.selected[i]!)}
    />
  );
}

function PreviewCard({ s }: { s: ImportState }) {
  const [selection, setSelection] = useState<GridSelection | null>(null);
  const [big, setBig] = useState(false);
  const m = s.preview;
  const ins = s.inspection;
  const facts = m ? summarize(m) : null;
  const assignee = ins?.assigneeCandidates[0];
  const range = s.form ? parseRange(s.form.range) : null;

  return (
    <section className="card preview" aria-label="미리보기">
      <div className="card-head">
        <b>미리보기{s.sheet ? ` · ${s.sheet}` : ""}</b>
        {s.stale && m && <span className="muted small">설정이 바뀌어 다시 읽는 중입니다…</span>}
        <span className="spacer" />
        <button
          className="ghost small"
          onClick={() => setBig(true)}
          disabled={!m}
          title="미리보기를 창 가득 크게 봅니다"
        >
          ⤢ 크게 보기
        </button>
      </div>
      {big && m && s.form && (
        <PreviewModal
          model={m}
          title={s.sheet ?? ""}
          stale={s.stale}
          selection={selection}
          onSelectionChange={setSelection}
          headerRows={s.form.headerRows}
          frozenCols={s.form.frozenCols}
          frozenMax={range ? Math.min(range.cols, MAX_FROZEN_COLS) : 0}
          onHeaderRows={(headerRows) => s.update({ headerRows })}
          onFrozenCols={(frozenCols) => s.update({ frozenCols })}
          tabs={<PreviewTabs s={s} />}
          onClose={() => setBig(false)}
        />
      )}
      {m && facts ? (
        <div className="preview-body">
          <div className="preview-main">
            <SheetGrid
              key={s.sheet ?? ""}
              model={m}
              selection={selection}
              onSelectionChange={setSelection}
              className={`preview-grid${s.stale ? " stale" : ""}`}
            />
            <PreviewTabs s={s} />
          </div>
          <div className="facts-col">
            <b>감지 결과</b>
            <dl className="facts">
              <dt>데이터 행</dt>
              <dd>{facts.dataRows.toLocaleString()}</dd>
              <dt>열</dt>
              <dd>{facts.cols}</dd>
              <dt>병합 영역</dt>
              <dd>{m.merges.length.toLocaleString()}</dd>
              <dt>수식 셀{columnsLabel(facts.formulaCols)}</dt>
              <dd>{facts.formulas.toLocaleString()}</dd>
              <dt>목록 유효성{columnsLabel(facts.listCols)}</dt>
              <dd>{facts.listCols.length}</dd>
              {(facts.unhiddenRows > 0 || facts.unhiddenCols > 0) && (
                <>
                  <dt title="Excel에서 숨겨져 있던 행·열은 펼쳐서 불러옵니다(참여자에게도 보입니다). 저장할 때 Excel에서도 숨김을 풉니다">
                    숨긴 행·열 (펼쳐서 불러옴)
                  </dt>
                  <dd>
                    {facts.unhiddenRows}행 · {facts.unhiddenCols}열
                  </dd>
                </>
              )}
            </dl>
            {assignee && (
              <div className="assignee small">
                {colLetters(assignee.col)}열 ‘{assignee.label}’ → 행 배정 기준 후보 · {assignee.distinct}명
              </div>
            )}
            {s.result && (
              <div className="muted small">
                읽기 {(s.result.stats.totalMs / 1000).toFixed(1)}초 · Excel 호출{" "}
                {s.result.stats.comCalls.toLocaleString()}회
              </div>
            )}
          </div>
        </div>
      ) : (
        <div className="placeholder muted">
          {s.busy === "read" || s.busy === "inspect" ? "읽는 중…" : "통합문서와 시트를 고르면 여기에 미리 보입니다."}
        </div>
      )}
    </section>
  );
}

/** 감지 결과 숫자(머리글은 데이터에서 뺀다 — 숨겨져 있던 행·열은 펼쳐 불러오므로 센다). 열 번호는 시트 기준. */
function summarize(m: SheetModel) {
  const dataRows = Math.max(0, m.rows - m.headerRows);
  const formulaCols = new Set<number>();
  let formulas = 0;
  for (let r = m.headerRows; r < m.rows; r++) {
    for (let c = 0; c < m.cols; c++) {
      if (m.cells[r * m.cols + c]?.f) {
        formulas++;
        formulaCols.add(m.origin.col + c);
      }
    }
  }
  const listCols = [
    ...new Set(m.validations.filter((v) => v.rule.kind === "list").map((v) => m.origin.col + v.range.col)),
  ];
  return {
    dataRows,
    cols: m.cols,
    unhiddenRows: m.unhiddenRows?.length ?? 0,
    unhiddenCols: m.unhiddenCols?.length ?? 0,
    formulas,
    formulaCols: [...formulaCols].sort((a, b) => a - b),
    listCols,
  };
}

/** 읽기 진행률 · 취소 · 오류. */
function ImportStatus({ s }: { s: ImportState }) {
  const p = s.progress;
  const cols = s.form ? (parseRange(s.form.range)?.cols ?? 1) : 1;
  const rows = p ? Math.round(p.total / cols) : 0;
  const doneRows = p ? Math.round(p.done / cols) : 0;
  const pct = p && p.total > 0 ? Math.round((p.done / p.total) * 100) : 0;
  const reading = s.busy === "read" || (s.busy === "commit" && s.step !== null);
  if (!reading && !s.error) return null;
  return (
    <div className="import-status" aria-live="polite">
      {reading ? (
        <div className="progress">
          {s.step ? `‘${s.step.sheet}’ 시트 읽는 중 (${s.step.n}/${s.step.of})` : "읽는 중"} ·{" "}
          {p ? STAGE_LABEL[p.stage] : "준비"}{" "}
          {p && (p.stage === "styles" || p.stage === "merges")
            ? `${doneRows.toLocaleString()} / ${rows.toLocaleString()}행`
            : ""}
          {p && (p.stage === "styles" || p.stage === "merges") ? (
            <span className="bar">
              <span style={{ width: `${pct}%` }} />
            </span>
          ) : (
            // 행 수를 모르는 단계: 멈춘 막대 대신 움직이는 막대로 읽는 중임을 보인다.
            <span className="bar indeterminate">
              <span />
            </span>
          )}
        </div>
      ) : (
        <span className="error">{s.error}</span>
      )}
      <span className="spacer" />
      {reading && (
        <button className="ghost small" onClick={s.cancel}>
          취소
        </button>
      )}
    </div>
  );
}
