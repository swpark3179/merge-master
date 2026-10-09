import type { GridSelection } from "@mm/grid";
import { SheetGrid } from "@mm/grid";
import type { ImportItems, ReadStage, SheetModel } from "@mm/shared";
import { colLetters, colSpanLabel, formatRange, parseRange } from "@mm/shared";
import { useEffect, useState } from "react";

import type { ImportState } from "./useImport";
import { useImport } from "./useImport";

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
  { key: "includeHidden", label: "숨긴 행·열 포함" },
];

/** 틀 고정 열 선택지는 앞쪽 몇 열까지만 둔다(참여자 화면이 좁아지지 않게). */
const MAX_FROZEN_COLS = 8;

interface Props {
  onExcelStatus: (running: boolean) => void;
  onCommitted: (m: SheetModel) => void;
}

/** 1a 엑셀 불러오기. */
export function ImportScreen({ onExcelStatus, onCommitted }: Props) {
  const s = useImport(onExcelStatus, onCommitted);
  const [didInit, setDidInit] = useState(false);
  useEffect(() => {
    if (didInit) return;
    setDidInit(true);
    void s.refresh();
  }, [didInit, s.refresh]);

  return (
    <div className="screen">
      <div className="screen-head">
        <h2>엑셀 불러오기</h2>
        <p className="muted">
          실행 중인 Excel에서 통합문서를 고르거나 파일을 엽니다. 값·서식·병합·유효성을 함께 읽습니다.
        </p>
      </div>
      <div className="import-grid">
        <div className="col">
          <WorkbookCard s={s} />
          {s.book && <SheetCard s={s} />}
        </div>
        <div className="col wide">
          <RangeCard s={s} />
          <ItemsCard s={s} />
          <PreviewCard s={s} />
        </div>
      </div>
      <ImportFooter s={s} />
    </div>
  );
}

function WorkbookCard({ s }: { s: ImportState }) {
  const books = s.status?.workbooks ?? [];
  return (
    <section className="card" aria-label="통합문서">
      <div className="card-head">
        <b>통합문서</b>
        <span className="spacer" />
        <button className="ghost small" onClick={() => void s.refresh()} disabled={s.busy !== null}>
          ↻ 다시 찾기
        </button>
      </div>
      <div className="segmented">
        <span className="on">실행 중인 Excel</span>
        <button onClick={() => void s.openFile()} disabled={s.busy !== null}>
          파일 열기…
        </button>
      </div>
      {s.busy === "list" && <div className="muted small">Excel을 찾는 중…</div>}
      {s.status && books.length === 0 && (
        <div className="muted small">
          {s.status.running
            ? "실행 중인 Excel에 열린 통합문서가 없습니다."
            : "실행 중인 Excel이 없습니다. ‘파일 열기…’로 불러오면 보이지 않는 Excel에서 읽습니다."}
        </div>
      )}
      {books.map((b) => (
        <button key={b.id} className={`book${b.id === s.book?.id ? " picked" : ""}`} onClick={() => s.pick(b)}>
          <span className="book-name">{b.name}</span>
          <span className="muted small">
            {b.path || "저장 안 됨"} · 시트 {b.sheets.length}개{b.background ? " · 백그라운드" : ""}
          </span>
        </button>
      ))}
    </section>
  );
}

function SheetCard({ s }: { s: ImportState }) {
  const lists = (s.inspection?.validations ?? []).filter((v) => v.rule.kind === "list");
  return (
    <section className="card" aria-label="시트">
      <b>시트</b>
      <div className="chips">
        {s.book!.sheets.map((name) => (
          <button
            key={name}
            className={`chip${name === s.sheet ? " on" : ""}`}
            onClick={() => s.setSheet(name)}
            disabled={s.busy === "inspect"}
          >
            {name === s.sheet ? "✓ " : ""}
            {name}
          </button>
        ))}
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
              ~
              <input
                type="number"
                min={start - 1}
                aria-label="머리글 끝 행"
                value={start + form.headerRows - 1}
                onChange={(e) => s.update({ headerRows: Math.max(0, Number(e.target.value) - start + 1) })}
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
  return (
    <section className="card" aria-label="가져올 항목">
      <b>가져올 항목</b>
      <div className="items">
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
    </section>
  );
}

function columnsLabel(cols: number[]): string {
  if (cols.length === 0) return "";
  const letters = cols.slice(0, 3).map(colLetters).join("·");
  return ` (${letters}${cols.length > 3 ? " 외" : ""}열)`;
}

function PreviewCard({ s }: { s: ImportState }) {
  const [selection, setSelection] = useState<GridSelection | null>(null);
  const m = s.preview;
  const ins = s.inspection;
  const facts = m ? summarize(m) : null;
  const assignee = ins?.assigneeCandidates[0];

  return (
    <section className="card preview" aria-label="미리보기">
      <div className="card-head">
        <b>미리보기{s.sheet ? ` · ${s.sheet}` : ""}</b>
        {s.stale && m && <span className="muted small">설정이 바뀌어 다시 읽는 중입니다…</span>}
      </div>
      {m && facts ? (
        <div className="preview-body">
          <SheetGrid
            model={m}
            selection={selection}
            onSelectionChange={setSelection}
            className={`preview-grid${s.stale ? " stale" : ""}`}
          />
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
              {(m.hiddenRows.length > 0 || m.hiddenCols.length > 0) && (
                <>
                  <dt>숨긴 행·열 (제외)</dt>
                  <dd>
                    {m.hiddenRows.length}행 · {m.hiddenCols.length}열
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

/** 감지 결과 숫자(숨긴 행·열과 머리글은 데이터에서 뺀다). 열 번호는 시트 기준. */
function summarize(m: SheetModel) {
  const hiddenRows = new Set(m.hiddenRows);
  let dataRows = 0;
  for (let r = m.headerRows; r < m.rows; r++) if (!hiddenRows.has(r)) dataRows++;
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
    cols: m.cols - m.hiddenCols.length,
    formulas,
    formulaCols: [...formulaCols].sort((a, b) => a - b),
    listCols,
  };
}

function ImportFooter({ s }: { s: ImportState }) {
  const p = s.progress;
  const cols = s.form ? (parseRange(s.form.range)?.cols ?? 1) : 1;
  const rows = p ? Math.round(p.total / cols) : 0;
  const doneRows = p ? Math.round(p.done / cols) : 0;
  const pct = p && p.total > 0 ? Math.round((p.done / p.total) * 100) : 0;
  return (
    <footer className="screen-foot">
      <div className="progress" aria-live="polite">
        {s.busy === "read" ? (
          <>
            읽는 중 · {p ? STAGE_LABEL[p.stage] : "준비"}{" "}
            {p && (p.stage === "styles" || p.stage === "merges")
              ? `${doneRows.toLocaleString()} / ${rows.toLocaleString()}행`
              : ""}
            <span className="bar">
              <span style={{ width: `${p && (p.stage === "styles" || p.stage === "merges") ? pct : 15}%` }} />
            </span>
          </>
        ) : (
          s.error && <span className="error">{s.error}</span>
        )}
      </div>
      <span className="spacer" />
      <button className="ghost" onClick={s.cancel} disabled={s.busy !== "read"}>
        취소
      </button>
      <button onClick={() => void s.commit()} disabled={!s.result || s.stale || s.busy !== null}>
        불러오기 → 취합 설정
      </button>
    </footer>
  );
}
