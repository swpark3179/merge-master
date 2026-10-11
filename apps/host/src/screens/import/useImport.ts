import type {
  ExcelStatus,
  ImportItems,
  ImportResult,
  ReadOptions,
  ReadProgress,
  SheetInspection,
  SheetModel,
  WorkbookInfo,
} from "@mm/shared";
import { formatRange, parseRange } from "@mm/shared";
import { open } from "@tauri-apps/plugin-dialog";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import { api, CANCELLED, errorText } from "../../api";

export const DEFAULT_ITEMS: ImportItems = {
  formulas: true,
  merges: true,
  fonts: true,
  borders: true,
  fills: true,
  dimensions: true,
  validations: true,
  includeHidden: false,
};

/** 1a 입력값(시트마다). 머리글·틀 고정은 다시 읽지 않고 반영하고, 범위·가져올 항목이 바뀌면 다시 읽는다. */
export interface ImportForm {
  range: string;
  /** 범위 첫 행부터 센 머리글 행 수. */
  headerRows: number;
  /** 범위 첫 열부터 센 틀 고정 열 수. */
  frozenCols: number;
  items: ImportItems;
}

/** 고른 시트 하나의 감지·입력값·읽기 결과. */
export interface SheetImport {
  inspection: SheetInspection | null;
  form: ImportForm | null;
  result: ImportResult | null;
  /** `result`를 읽을 때의 범위·항목(지금 입력값과 다르면 다시 읽어야 한다). */
  resultKey: string | null;
}

const EMPTY: SheetImport = { inspection: null, form: null, result: null, resultKey: null };

export const EXCEL_EXTENSIONS = ["xlsx", "xlsm", "xlsb", "xls"];

type Busy = "list" | "open" | "inspect" | "read" | "commit" | null;

/** 범위·항목을 고친 뒤 다시 읽기까지 기다리는 시간(입력 중에는 읽지 않는다). */
const REREAD_DELAY_MS = 600;

function formOf(ins: SheetInspection, items: ImportItems): ImportForm {
  return {
    range: formatRange(ins.table),
    headerRows: ins.headerRows,
    frozenCols: Math.min(Math.max(ins.frozenCols - ins.table.col, 0), ins.table.cols),
    items,
  };
}

/** 읽기 결과가 맞는지 보는 열쇠: 통합문서·시트·범위·가져올 항목. 범위가 잘못되었으면 `null`. */
function readKey(bookId: string, sheet: string, form: ImportForm): string | null {
  const range = parseRange(form.range);
  return range ? JSON.stringify([bookId, sheet, formatRange(range), form.items]) : null;
}

function readOptions(form: ImportForm): ReadOptions {
  return {
    range: formatRange(parseRange(form.range)!),
    headerRows: form.headerRows,
    frozenCols: form.frozenCols,
    items: form.items,
  };
}

/**
 * 1a 불러오기. 통합문서에서 시트를 하나 이상 고르고(체크), 미리 보는 시트(초점)의 범위·머리글·틀 고정을 고친다.
 * 읽기는 한 번에 하나(Excel 스레드)라 초점 시트만 바로 읽고, 확정할 때 아직 읽지 않았거나 낡은 시트를 차례로 읽는다.
 */
export function useImport(onExcelStatus: (running: boolean) => void, onCommitted: (m: SheetModel[]) => void) {
  const [status, setStatus] = useState<ExcelStatus | null>(null);
  const [bookId, setBookId] = useState<string | null>(null);
  /** 함께 채울 시트(통합문서 순서). */
  const [selected, setSelected] = useState<string[]>([]);
  /** 미리 보고 범위를 고치는 시트(고른 시트 중 하나). */
  const [sheet, setFocus] = useState<string | null>(null);
  const [per, setPer] = useState<Record<string, SheetImport>>({});
  const [progress, setProgress] = useState<ReadProgress | null>(null);
  /** 확정하며 다른 시트를 읽는 중이면 그 시트와 몇 번째인지. */
  const [step, setStep] = useState<{ sheet: string; n: number; of: number } | null>(null);
  const [busy, setBusy] = useState<Busy>(null);
  const [error, setError] = useState<string | null>(null);
  const readToken = useRef(0);
  const readNow = useRef(false);
  const itemsRef = useRef<ImportItems>(DEFAULT_ITEMS);
  const perRef = useRef(per);
  perRef.current = per;

  const book = status?.workbooks.find((b) => b.id === bookId) ?? null;
  const cur = (sheet && per[sheet]) || EMPTY;

  const patch = useCallback((name: string, p: Partial<SheetImport>) => {
    setPer((all) => ({ ...all, [name]: { ...(all[name] ?? EMPTY), ...p } }));
  }, []);

  const pick = useCallback((b: WorkbookInfo) => {
    const first = b.activeSheet || b.sheets[0] || null;
    setBookId(b.id);
    setPer({});
    setSelected(first ? [first] : []);
    setFocus(first);
  }, []);

  /** '다른 파일': 고른 통합문서를 놓고 목록으로 돌아간다. */
  const unpick = useCallback(() => {
    readToken.current++;
    setBookId(null);
    setSelected([]);
    setFocus(null);
    setPer({});
  }, []);

  const refresh = useCallback(async () => {
    setBusy("list");
    setError(null);
    try {
      const st = await api.listWorkbooks();
      setStatus(st);
      onExcelStatus(st.running);
      // 고른 통합문서가 아직 열려 있으면 그대로 두고, 아니면 사용자가 다시 고르게 한다.
      setBookId((current) => {
        if (current && st.workbooks.some((b) => b.id === current)) return current;
        setSelected([]);
        setFocus(null);
        setPer({});
        return null;
      });
    } catch (e) {
      setError(errorText(e));
    } finally {
      setBusy(null);
    }
  }, [onExcelStatus]);

  /** `dropped`가 없으면 파일 대화상자로 고른다. */
  const openFile = useCallback(
    async (dropped?: string) => {
      const path =
        dropped ??
        (await open({
          multiple: false,
          filters: [{ name: "Excel 통합문서", extensions: EXCEL_EXTENSIONS }],
        }));
      if (typeof path !== "string") return;
      setBusy("open");
      setError(null);
      try {
        const info = await api.openFile(path);
        const st = await api.listWorkbooks();
        setStatus(st);
        onExcelStatus(st.running);
        pick(info);
      } catch (e) {
        setError(errorText(e));
      } finally {
        setBusy(null);
      }
    },
    [onExcelStatus, pick],
  );

  /** 통합문서 순서로 정렬. */
  const ordered = useCallback(
    (names: string[]) => (book ? book.sheets.filter((n) => names.includes(n)) : names),
    [book],
  );

  /** 미리 볼 시트를 바꾼다(아직 고르지 않은 시트면 함께 고른다). */
  const setSheet = useCallback(
    (name: string) => {
      setSelected((list) => (list.includes(name) ? list : ordered([...list, name])));
      setFocus(name);
    },
    [ordered],
  );

  /** 시트를 고르거나 뺀다(하나는 남긴다). 미리 보던 시트를 빼면 남은 첫 시트를 본다. */
  const toggle = useCallback(
    (name: string) => {
      if (!selected.includes(name)) {
        setSelected(ordered([...selected, name]));
        setFocus(name);
        return;
      }
      if (selected.length <= 1) return;
      const rest = selected.filter((n) => n !== name);
      setSelected(rest);
      if (sheet === name) setFocus(rest[0] ?? null);
    },
    [selected, sheet, ordered],
  );

  // 처음 보는 시트는 사전 감지 → 감지값으로 바로 읽는다.
  const inspected = sheet ? per[sheet]?.inspection !== undefined && per[sheet]?.inspection !== null : false;
  useEffect(() => {
    if (!bookId || !sheet || inspected) return;
    let alive = true;
    readToken.current++;
    setBusy("inspect");
    setError(null);
    api
      .inspectSheet(bookId, sheet)
      .then((ins) => {
        if (!alive) return;
        readNow.current = true;
        patch(sheet, { inspection: ins, form: formOf(ins, itemsRef.current), result: null, resultKey: null });
      })
      .catch((e) => alive && setError(errorText(e)))
      .finally(() => alive && setBusy(null));
    return () => {
      alive = false;
    };
  }, [bookId, sheet, inspected, patch]);

  const form = cur.form;
  const key = form && bookId && sheet ? readKey(bookId, sheet, form) : null;

  const read = useCallback(
    async (name: string, k: string, options: ReadOptions) => {
      if (!bookId) return;
      const token = ++readToken.current;
      setBusy("read");
      setProgress(null);
      setError(null);
      try {
        const r = await api.readSheet(bookId, name, options, (p) => {
          if (token === readToken.current) setProgress(p);
        });
        if (token !== readToken.current) return;
        patch(name, { result: r, resultKey: k });
      } catch (e) {
        if (token !== readToken.current) return;
        const msg = errorText(e);
        if (msg !== CANCELLED) setError(msg);
      } finally {
        if (token === readToken.current) {
          setBusy(null);
          setProgress(null);
        }
      }
    },
    [bookId, patch],
  );

  // 범위·항목이 바뀌면(입력이 멈춘 뒤) 다시 읽는다. 뒤의 읽기가 앞선 읽기를 취소한다.
  useEffect(() => {
    if (!key || !form || !sheet || key === cur.resultKey) return;
    const options = readOptions(form);
    const delay = readNow.current ? 0 : REREAD_DELAY_MS;
    readNow.current = false;
    const name = sheet;
    const t = window.setTimeout(() => void read(name, key, options), delay);
    return () => window.clearTimeout(t);
    // 의존성은 key(시트·범위·항목)만 둔다: 머리글·틀 고정만 바뀌었을 때는 다시 읽지 않는다.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [key]);

  const cancel = useCallback(() => {
    readToken.current++;
    void api.cancelRead();
    setBusy(null);
    setProgress(null);
  }, []);

  /** 미리 보는 시트의 입력값을 고친다. */
  const update = useCallback(
    (p: Partial<ImportForm>) => {
      if (!sheet) return;
      setPer((all) => {
        const s = all[sheet];
        if (!s?.form) return all;
        const next = { ...s.form, ...p };
        itemsRef.current = next.items;
        return { ...all, [sheet]: { ...s, form: next } };
      });
    },
    [sheet],
  );

  /** 미리보기에는 머리글·틀 고정 조정을 바로 반영한다. */
  const previewOf = useCallback((s: SheetImport | undefined): SheetModel | null => {
    if (!s?.result || !s.form) return null;
    const m = s.result.model;
    return {
      ...m,
      headerRows: Math.min(Math.max(s.form.headerRows, 0), m.rows),
      frozenCols: Math.min(Math.max(s.form.frozenCols, 0), m.cols),
    };
  }, []);
  const preview = useMemo(() => previewOf(cur), [cur, previewOf]);

  const stale = key !== null && key !== cur.resultKey;

  /** 마지막으로 확정한 시트들의 읽기 결과 + 머리글·틀 고정. 같으면 다시 확정하지 않는다(설정이 초기화되지 않게). */
  const [committedSig, setCommittedSig] = useState<string | null>(null);
  const sig = JSON.stringify(
    selected.map((n) => [n, per[n]?.resultKey ?? null, per[n]?.form?.headerRows, per[n]?.form?.frozenCols]),
  );
  const changedSinceCommit = sig !== committedSig;

  /** 고른 시트를 모두 확정한다: 아직 감지·읽기를 안 했거나 낡은 시트는 차례로 읽는다. */
  const commit = useCallback(async () => {
    if (!bookId || selected.length === 0) return;
    setBusy("commit");
    setError(null);
    try {
      const done: Record<string, SheetImport> = { ...perRef.current };
      for (const [i, name] of selected.entries()) {
        let s = done[name] ?? EMPTY;
        if (!s.inspection || !s.form) {
          setStep({ sheet: name, n: i + 1, of: selected.length });
          const ins = await api.inspectSheet(bookId, name);
          s = { ...s, inspection: ins, form: formOf(ins, itemsRef.current) };
        }
        const k = readKey(bookId, name, s.form!);
        if (!k) throw new Error(`‘${name}’ 시트의 데이터 범위를 확인해 주세요`);
        if (k !== s.resultKey || !s.result) {
          setStep({ sheet: name, n: i + 1, of: selected.length });
          const token = ++readToken.current;
          const r = await api.readSheet(bookId, name, readOptions(s.form!), (p) => {
            if (token === readToken.current) setProgress(p);
          });
          s = { ...s, result: r, resultKey: k };
        }
        done[name] = s;
      }
      setPer(done);
      setStep(null);
      setProgress(null);
      const models = await api.commitImport(
        bookId,
        selected.map((name) => ({
          sheet: name,
          headerRows: done[name]!.form!.headerRows,
          frozenCols: done[name]!.form!.frozenCols,
        })),
      );
      setCommittedSig(
        JSON.stringify(
          selected.map((n) => [n, done[n]?.resultKey ?? null, done[n]?.form?.headerRows, done[n]?.form?.frozenCols]),
        ),
      );
      onCommitted(models);
    } catch (e) {
      const msg = errorText(e);
      if (msg !== CANCELLED) setError(msg);
    } finally {
      setStep(null);
      setProgress(null);
      setBusy(null);
    }
  }, [bookId, selected, onCommitted]);

  return {
    status,
    book,
    /** 미리 보는 시트. */
    sheet,
    setSheet,
    selected,
    toggle,
    /** 시트마다 감지·읽기 상태. */
    per,
    pick,
    unpick,
    refresh,
    openFile,
    inspection: cur.inspection,
    form,
    update,
    rangeValid: form ? parseRange(form.range) !== null : false,
    result: cur.result,
    preview,
    previewOf,
    stale,
    progress,
    step,
    busy,
    error,
    cancel,
    commit,
    changedSinceCommit,
  };
}

export type ImportState = ReturnType<typeof useImport>;
