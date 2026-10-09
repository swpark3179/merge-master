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

/** 1a 입력값. 머리글·틀 고정은 다시 읽지 않고 반영하고, 범위·가져올 항목이 바뀌면 다시 읽는다. */
export interface ImportForm {
  range: string;
  /** 범위 첫 행부터 센 머리글 행 수. */
  headerRows: number;
  /** 범위 첫 열부터 센 틀 고정 열 수. */
  frozenCols: number;
  items: ImportItems;
}

type Busy = "list" | "open" | "inspect" | "read" | "commit" | null;

/** 범위·항목을 고친 뒤 다시 읽기까지 기다리는 시간(입력 중에는 읽지 않는다). */
const REREAD_DELAY_MS = 600;

export function useImport(onExcelStatus: (running: boolean) => void, onCommitted: (m: SheetModel) => void) {
  const [status, setStatus] = useState<ExcelStatus | null>(null);
  const [bookId, setBookId] = useState<string | null>(null);
  const [sheet, setSheet] = useState<string | null>(null);
  const [inspection, setInspection] = useState<SheetInspection | null>(null);
  const [form, setForm] = useState<ImportForm | null>(null);
  const [result, setResult] = useState<ImportResult | null>(null);
  const [resultKey, setResultKey] = useState<string | null>(null);
  const [progress, setProgress] = useState<ReadProgress | null>(null);
  const [busy, setBusy] = useState<Busy>(null);
  const [error, setError] = useState<string | null>(null);
  const readToken = useRef(0);
  const readNow = useRef(false);
  const itemsRef = useRef<ImportItems>(DEFAULT_ITEMS);

  const book = status?.workbooks.find((b) => b.id === bookId) ?? null;

  const pick = useCallback((b: WorkbookInfo) => {
    setBookId(b.id);
    setSheet(b.activeSheet || b.sheets[0] || null);
  }, []);

  const refresh = useCallback(async () => {
    setBusy("list");
    setError(null);
    try {
      const st = await api.listWorkbooks();
      setStatus(st);
      onExcelStatus(st.running);
      setBookId((current) => {
        if (current && st.workbooks.some((b) => b.id === current)) return current;
        const first = st.workbooks[0];
        setSheet(first ? first.activeSheet || first.sheets[0] || null : null);
        return first?.id ?? null;
      });
    } catch (e) {
      setError(errorText(e));
    } finally {
      setBusy(null);
    }
  }, [onExcelStatus]);

  const openFile = useCallback(async () => {
    const path = await open({
      multiple: false,
      filters: [{ name: "Excel 통합문서", extensions: ["xlsx", "xlsm", "xlsb", "xls"] }],
    });
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
  }, [onExcelStatus, pick]);

  // 시트를 고르면 사전 감지 → 감지값으로 바로 읽는다.
  useEffect(() => {
    if (!bookId || !sheet) return;
    let alive = true;
    readToken.current++;
    setBusy("inspect");
    setError(null);
    setInspection(null);
    // 이전 시트의 범위로 새 시트를 읽지 않도록 입력값도 비운다(감지가 끝나면 다시 채운다).
    setForm(null);
    setResult(null);
    setResultKey(null);
    api
      .inspectSheet(bookId, sheet)
      .then((ins) => {
        if (!alive) return;
        setInspection(ins);
        readNow.current = true;
        setForm({
          range: formatRange(ins.table),
          headerRows: ins.headerRows,
          frozenCols: Math.min(Math.max(ins.frozenCols - ins.table.col, 0), ins.table.cols),
          items: itemsRef.current,
        });
      })
      .catch((e) => alive && setError(errorText(e)))
      .finally(() => alive && setBusy(null));
    return () => {
      alive = false;
    };
  }, [bookId, sheet]);

  const parsedRange = form ? parseRange(form.range) : null;
  const key =
    form && bookId && sheet && parsedRange
      ? JSON.stringify([bookId, sheet, formatRange(parsedRange), form.items])
      : null;

  const read = useCallback(
    async (readKey: string, options: ReadOptions) => {
      if (!bookId || !sheet) return;
      const token = ++readToken.current;
      setBusy("read");
      setProgress(null);
      setError(null);
      try {
        const r = await api.readSheet(bookId, sheet, options, (p) => {
          if (token === readToken.current) setProgress(p);
        });
        if (token !== readToken.current) return;
        setResult(r);
        setResultKey(readKey);
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
    [bookId, sheet],
  );

  // 범위·항목이 바뀌면(입력이 멈춘 뒤) 다시 읽는다. 뒤의 읽기가 앞선 읽기를 취소한다.
  useEffect(() => {
    if (!key || !form || !parsedRange || key === resultKey) return;
    const options: ReadOptions = {
      range: formatRange(parsedRange),
      headerRows: form.headerRows,
      frozenCols: form.frozenCols,
      items: form.items,
    };
    const delay = readNow.current ? 0 : REREAD_DELAY_MS;
    readNow.current = false;
    const t = window.setTimeout(() => void read(key, options), delay);
    return () => window.clearTimeout(t);
    // 의존성은 key(범위·항목)만 둔다: 머리글·틀 고정만 바뀌었을 때는 다시 읽지 않는다.
  }, [key]);

  const cancel = useCallback(() => {
    readToken.current++;
    void api.cancelRead();
    setBusy(null);
    setProgress(null);
  }, []);

  const update = useCallback((patch: Partial<ImportForm>) => {
    setForm((f) => {
      if (!f) return f;
      const next = { ...f, ...patch };
      itemsRef.current = next.items;
      return next;
    });
  }, []);

  /** 미리보기에는 머리글·틀 고정 조정을 바로 반영한다. */
  const preview = useMemo<SheetModel | null>(() => {
    if (!result || !form) return null;
    const m = result.model;
    return {
      ...m,
      headerRows: Math.min(Math.max(form.headerRows, 0), m.rows),
      frozenCols: Math.min(Math.max(form.frozenCols, 0), m.cols),
    };
  }, [result, form]);

  const stale = key !== null && key !== resultKey;

  const commit = useCallback(async () => {
    if (!form || stale) return;
    setBusy("commit");
    try {
      onCommitted(await api.commitImport(form.headerRows, form.frozenCols));
    } catch (e) {
      setError(errorText(e));
    } finally {
      setBusy(null);
    }
  }, [form, stale, onCommitted]);

  return {
    status,
    book,
    sheet,
    setSheet,
    pick,
    refresh,
    openFile,
    inspection,
    form,
    update,
    rangeValid: parsedRange !== null,
    result,
    preview,
    stale,
    progress,
    busy,
    error,
    cancel,
    commit,
  };
}

export type ImportState = ReturnType<typeof useImport>;
