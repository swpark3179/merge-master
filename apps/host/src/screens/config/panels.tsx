import type { CellRange, ColumnRule, ExceptionKind, InputFormat, Permissions } from "@mm/shared";
import { colLetters, formatRange, personColor } from "@mm/shared";
import { useEffect, useState } from "react";

import { api } from "../../api";
import type { ConfigState } from "./useConfig";

function sheetRange(s: ConfigState, r: CellRange): string {
  return formatRange({ ...r, row: r.row + s.model.origin.row, col: r.col + s.model.origin.col });
}

function letter(s: ConfigState, col: number): string {
  return colLetters(s.model.origin.col + col);
}

const KIND_LABEL: Record<ExceptionKind, string> = { locked: "입력 금지", input: "입력 가능", required: "필수 추가" };

function defaultHint(f: InputFormat): string {
  switch (f.kind) {
    case "number":
      return f.unit ? `${f.unit} 단위, 숫자만` : "숫자만";
    case "list":
      return "목록에서 고르세요";
    case "date":
      return "날짜 (예: 2026-10-10)";
    default:
      return "";
  }
}

/** 활성 칸이 있는 열의 역할(입력/잠금)·필수(입력일 때만)·조회조건·형식·안내. */
export function ColumnPanel({ s }: { s: ConfigState }) {
  const col = s.selection?.anchor.col;
  if (col === undefined) {
    return (
      <section className="side-card">
        <b>열 속성</b>
        <div className="muted small">그리드에서 칸이나 열 머리글을 고르세요.</div>
      </section>
    );
  }
  // 열이 바뀌면 입력 중이던 값을 버린다.
  return <ColumnEditor key={col} s={s} col={col} />;
}

function ColumnEditor({ s, col }: { s: ConfigState; col: number }) {
  const rule = s.view!.config.columns[col]!;
  const [unit, setUnit] = useState(rule.format.kind === "number" ? rule.format.unit : "");
  const [items, setItems] = useState(rule.format.kind === "list" ? rule.format.items.join(", ") : "");
  const [hint, setHint] = useState(rule.hint);
  const set = (patch: Partial<ColumnRule>) => void s.apply({ op: "setColumn", col, rule: { ...rule, ...patch } });
  const setFormat = (format: InputFormat) =>
    set({ format, hint: hint === defaultHint(rule.format) ? defaultHint(format) : hint });

  const kind = rule.format.kind;
  return (
    <section className="side-card" aria-label="열 속성">
      <div className="card-head">
        <span className="col-badge">{letter(s, col)}</span>
        <b>{s.view!.labels[col] || `${letter(s, col)}열`}</b>
        <span className="muted small">열 전체</span>
      </div>
      <div className="segmented" role="group" aria-label="열 역할">
        <button
          className={rule.role === "input" ? "on" : ""}
          aria-pressed={rule.role === "input"}
          onClick={() => set({ role: "input" })}
        >
          입력
        </button>
        <button
          className={rule.role === "locked" ? "on" : ""}
          aria-pressed={rule.role === "locked"}
          onClick={() => set({ role: "locked", required: false })}
        >
          잠금
        </button>
      </div>
      <div className="row-checks">
        {rule.role === "input" && (
          <label className="check">
            <input
              type="checkbox"
              aria-label="필수"
              checked={rule.required}
              onChange={(e) => set({ required: e.target.checked })}
            />{" "}
            필수
          </label>
        )}
        <label className="check">
          <input type="checkbox" checked={rule.filter} onChange={(e) => set({ filter: e.target.checked })} /> 조회조건
        </label>
      </div>
      <div className="kv">
        <span className="muted">형식</span>
        <select
          aria-label="입력 형식"
          value={kind}
          onChange={(e) => {
            const k = e.target.value as InputFormat["kind"];
            if (k === "number") setFormat({ kind: "number", unit });
            else if (k === "list") setFormat({ kind: "list", items: splitItems(items) });
            else setFormat({ kind: k } as InputFormat);
          }}
        >
          <option value="text">글자</option>
          <option value="number">숫자</option>
          <option value="list">목록</option>
          <option value="date">날짜</option>
        </select>
        {kind === "number" && (
          <input
            aria-label="단위"
            className="unit"
            placeholder="단위"
            value={unit}
            onChange={(e) => setUnit(e.target.value)}
            onBlur={() =>
              unit !== (rule.format.kind === "number" ? rule.format.unit : "") && setFormat({ kind: "number", unit })
            }
          />
        )}
      </div>
      {kind === "list" && (
        <label className="field">
          목록 항목 (쉼표로 구분)
          <input
            aria-label="목록 항목"
            value={items}
            onChange={(e) => setItems(e.target.value)}
            onBlur={() => setFormat({ kind: "list", items: splitItems(items) })}
          />
        </label>
      )}
      <div className="kv">
        <span className="muted">안내</span>
        <input
          aria-label="안내"
          placeholder="참여자에게 보일 안내"
          value={hint}
          onChange={(e) => setHint(e.target.value)}
          onBlur={() => hint !== rule.hint && set({ hint })}
        />
      </div>
    </section>
  );
}

function splitItems(text: string): string[] {
  return text
    .split(/[,，\n]/)
    .map((t) => t.trim())
    .filter(Boolean);
}

export function ExceptionsPanel({ s }: { s: ConfigState }) {
  const list = s.view!.config.exceptions;
  const [kind, setKind] = useState<ExceptionKind>("locked");
  return (
    <section className="side-card" aria-label="셀 예외">
      <b>셀 예외</b>
      {list.length === 0 && (
        <div className="muted small">열 역할 위에 셀 단위로 덮어씁니다. 입력 금지가 항상 우선.</div>
      )}
      {list.map((e, i) => (
        <div key={`${formatRange(e.range)}-${i}`} className="exception-row">
          <span className="mono small">{sheetRange(s, e.range)}</span>
          <span className="small">{KIND_LABEL[e.kind]}</span>
          <input
            className="note"
            aria-label="예외 메모"
            defaultValue={e.note}
            placeholder="메모"
            onBlur={(ev) =>
              ev.target.value !== e.note && void s.apply({ op: "updateException", index: i, note: ev.target.value })
            }
          />
          <button
            className="x"
            aria-label="예외 삭제"
            onClick={() => void s.apply({ op: "removeException", index: i })}
          >
            ✕
          </button>
        </div>
      ))}
      <div className="add-row">
        <select aria-label="예외 종류" value={kind} onChange={(e) => setKind(e.target.value as ExceptionKind)}>
          <option value="locked">입력 금지</option>
          <option value="input">입력 가능</option>
          <option value="required">필수 추가</option>
        </select>
        <button
          className="link"
          disabled={!s.range || s.busy}
          onClick={() => s.range && void s.apply({ op: "addException", range: s.range, kind, note: "" })}
        >
          + 선택 셀을 예외로
        </button>
      </div>
    </section>
  );
}

/**
 * 담당자 배정 탭 위쪽: 첫 줄은 행 배정 기준 열(그 열의 이름으로 접속한 사람이 그 행을 맡음), 둘째 줄은 고른 칸의 담당자.
 * 행 배정도 범위 배정도 없는 입력 칸은 누구나 입력한다.
 */
export function AssignBar({ s }: { s: ConfigState }) {
  const view = s.view!;
  const { assignment } = view.config;
  const [person, setPerson] = useState("");
  const [note, setNote] = useState("");
  const [known, setKnown] = useState<string[]>([]);
  const name = person.trim();
  const suggested = view.suggestedKeyCol;
  const key = assignment.keyCol;
  // 담당자 후보: 명단 + IP 명단의 이름(아직 접속하지 않은 사람도 고를 수 있게).
  useEffect(() => {
    api
      .ipNamesGet()
      .then((v) => setKnown(v.entries.map((e) => e.name)))
      .catch(() => {});
  }, []);
  const candidates = [...new Set([...view.roster, ...known])];
  const assign = () => {
    if (!s.range || !name) return;
    void s.apply({ op: "assignRange", range: s.range, person: name, note });
    setNote("");
  };
  return (
    <div className="role-bar assign-bar" role="toolbar" aria-label="담당자 배정">
      <div className="assign-row">
        <label className="assign-field" title="이 열의 이름과 같은 이름으로 접속한 사람이 그 행을 맡습니다">
          <b>행 배정 기준</b>
          <select
            aria-label="행 배정 기준"
            value={key ?? ""}
            disabled={s.busy}
            onChange={(e) =>
              void s.apply({ op: "setKeyColumn", col: e.target.value === "" ? undefined : Number(e.target.value) })
            }
          >
            <option value="">행 배정 없음</option>
            {view.labels.map((label, c) => (
              <option key={c} value={c}>
                {letter(s, c)}열 {label || ""}
                {c === suggested ? " (추천)" : ""}
              </option>
            ))}
          </select>
        </label>
        <span className="role-hint">
          {key !== undefined && key !== null
            ? `${letter(s, key)}열의 이름과 같은 이름으로 접속한 사람이 그 행을 맡습니다. 이름이 빈 행은 누구나 입력합니다.`
            : "행을 사람에게 나누지 않습니다. 아래에서 범위를 배정하지 않은 입력 칸은 누구나 입력합니다."}
        </span>
      </div>
      <div className="assign-row">
        <b className="assign-label">선택 셀 담당자</b>
        <span className="assign-range mono" aria-label="고른 범위">
          {s.range ? sheetRange(s, s.range) : "칸을 고르세요"}
        </span>
        <input
          className="assign-person"
          aria-label="배정할 사람"
          list="config-roster"
          placeholder="이름 선택·입력"
          value={person}
          onChange={(e) => setPerson(e.target.value)}
          onKeyDown={(e) => e.key === "Enter" && !e.nativeEvent.isComposing && assign()}
        />
        <datalist id="config-roster">
          {candidates.map((p) => (
            <option key={p} value={p} />
          ))}
        </datalist>
        <input
          className="assign-note"
          aria-label="배정 메모"
          placeholder="메모"
          value={note}
          onChange={(e) => setNote(e.target.value)}
        />
        <button className="ghost role" disabled={!s.range || !name || s.busy} onClick={assign}>
          선택 범위 배정
        </button>
      </div>
    </div>
  );
}

/** 담당자 배정 탭 오른쪽: 명단(행 배정 기준 열의 이름 + 추가한 이름 + 범위 배정 받은 이름). */
export function RosterPanel({ s }: { s: ConfigState }) {
  const view = s.view!;
  const { assignment } = view.config;
  const [newName, setNewName] = useState("");
  return (
    <section className="side-card" aria-label="명단">
      <div className="card-head">
        <b>명단</b>
        <span className="muted small">{view.roster.length}명</span>
      </div>
      {view.roster.length === 0 && (
        <div className="muted small">
          아직 배정한 사람이 없습니다(입력 칸은 누구나 입력). 위에서 행 배정 기준 열을 고르거나, 범위를 사람에게
          배정하거나, 이름을 추가하세요.
        </div>
      )}
      {view.roster.length > 0 && (
        <div className="roster">
          {view.roster.map((p, i) => (
            <span key={p} className="person">
              <span className="mmg-dot" style={{ background: personColor(i) }} />
              {p}
              {assignment.extraPeople.includes(p) && (
                <button
                  className="x"
                  aria-label={`${p} 빼기`}
                  onClick={() => void s.apply({ op: "removePerson", name: p })}
                >
                  ✕
                </button>
              )}
            </span>
          ))}
        </div>
      )}
      <form
        className="add-row"
        onSubmit={(e) => {
          e.preventDefault();
          if (!newName.trim()) return;
          void s.apply({ op: "addPerson", name: newName });
          setNewName("");
        }}
      >
        <input
          aria-label="추가할 이름"
          placeholder="이름 추가"
          value={newName}
          onChange={(e) => setNewName(e.target.value)}
        />
        <button className="ghost small" type="submit">
          추가
        </button>
      </form>
    </section>
  );
}

/** 담당자 배정 탭 오른쪽: 범위 배정 목록(행 배정에 더한다). */
export function RangeAssignmentsPanel({ s }: { s: ConfigState }) {
  const view = s.view!;
  const { ranges } = view.config.assignment;
  return (
    <section className="side-card" aria-label="범위 배정">
      <div className="card-head">
        <b>범위 배정</b>
        <span className="muted small">행 배정에 더함</span>
      </div>
      {ranges.length === 0 && (
        <div className="muted small">그리드에서 범위를 고르고 위쪽 둘째 줄의 ‘선택 범위 배정’을 누르세요.</div>
      )}
      {ranges.map((a, i) => (
        <div key={`${formatRange(a.range)}-${i}`} className="exception-row">
          <span className="mmg-dot" style={{ background: personColor(view.roster.indexOf(a.person)) }} />
          <span className="mono small">{sheetRange(s, a.range)}</span>→ {a.person}
          {a.note && <span className="muted small">({a.note})</span>}
          <span className="spacer" />
          <button
            className="x"
            aria-label="범위 배정 삭제"
            onClick={() => void s.apply({ op: "removeAssignment", index: i })}
          >
            ✕
          </button>
        </div>
      ))}
      <div className="formula-note small">
        입력 가능 = (배정 행 ∪ 배정 범위) ∩ 입력 영역 − 입력 금지 · 담당자가 아무도 없는 입력 칸은 누구나
      </div>
    </section>
  );
}

export function PermissionsPanel({ s }: { s: ConfigState }) {
  const p = s.view!.config.permissions;
  const set = (patch: Partial<Permissions>) => void s.apply({ op: "setPermissions", permissions: { ...p, ...patch } });
  return (
    <section className="side-card" aria-label="참여자 권한">
      <b>참여자 권한</b>
      <div className="kv">
        <span className="muted">서식 변경</span>
        <div className="segmented">
          {(
            [
              ["none", "불가"],
              ["fillOnly", "배경색만"],
              ["all", "허용"],
            ] as const
          ).map(([v, label]) => (
            <button key={v} className={p.format === v ? "on" : ""} onClick={() => set({ format: v })}>
              {label}
            </button>
          ))}
        </div>
      </div>
      <div className="kv">
        <span className="muted">남의 행</span>
        <div className="segmented">
          {(
            [
              ["readOnly", "읽기 전용"],
              ["hidden", "숨김"],
            ] as const
          ).map(([v, label]) => (
            <button key={v} className={p.othersRows === v ? "on" : ""} onClick={() => set({ othersRows: v })}>
              {label}
            </button>
          ))}
        </div>
      </div>
      <div className="row-checks">
        <label className="check">
          <input type="checkbox" checked={p.filterSort} onChange={(e) => set({ filterSort: e.target.checked })} />{" "}
          필터·정렬
        </label>
        <label className="check">
          <input type="checkbox" checked={p.addRows} onChange={(e) => set({ addRows: e.target.checked })} /> 행 추가
        </label>
      </div>
    </section>
  );
}
