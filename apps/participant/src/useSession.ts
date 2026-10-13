import type {
  Axis,
  BatchOrigin,
  CellRange,
  CellRef,
  ChangeEntry,
  ChatMessage,
  ChatReaction,
  ChatTo,
  ClientMsg,
  HighlightView,
  KickReason,
  NoticeView,
  PersonProgress,
  PresenceEntry,
  ServerMsg,
  SessionInfo,
  SheetCell,
  StylePatch,
  Welcome,
} from "@mm/shared";
import { a1, decodeFlags, FLASH_MS, sheetPrefix } from "@mm/shared";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import type { Batch, CellChange } from "./edits";
import { EditState } from "./edits";
import type { StoredBatch } from "./outbox";
import { loadOutbox, outboxKey, saveOutbox, splitBySheet } from "./outbox";
import { storageSlug } from "./slug";

/** 서버 `protocol::PROTOCOL_VERSION`과 같아야 한다. */
const PROTOCOL = 8;
const RETRY_MS = [500, 1000, 2000, 4000, 8000];
const PING_MS = 20_000;
const CURSOR_MS = 100;

export type Phase =
  | { kind: "name"; error?: string }
  /** `known`: 이 PC가 호스트의 IP 명단에 있어 그 이름으로 바로 들어가는 중. */
  | { kind: "joining"; name: string; known?: boolean }
  | { kind: "pending"; name: string }
  | { kind: "dup"; name: string }
  | { kind: "in" }
  | { kind: "kicked"; reason: KickReason }
  /** 호스트가 서비스를 중지함(다시 열리면 자동으로 붙는다). `deadline`: 마감 시각이 되어 공유가 닫혔다. */
  | { kind: "closed"; deadline?: boolean }
  /** 호스트가 이 업무를 끝냈다(다시 붙지 않는다). */
  | { kind: "ended" };

/** 다른 사람이 방금 바꾼 칸: 바꾼 사람과 번호(같은 칸을 또 바꾸면 번호가 바뀌어 다시 깜빡인다). */
export interface RecentEdit {
  by: string;
  seq: number;
}

export interface Notice {
  id: number;
  text: string;
  tone: "info" | "warn" | "error";
  /** 알림 안 버튼(예: 강조한 셀로 이동). `sheet`가 있으면 그 시트로 가서. */
  action?: { label: string; range: CellRange; highlight?: number; sheet?: number };
}

/** 편집 잠금 알림: 내가 잡은 결과(받음·남이 쓰는 중), 또는 어떤 칸의 잠금이 풀렸다(남이 끝냄·호스트가 풂). */
export interface LockEvent {
  seq: number;
  kind: "granted" | "denied" | "released";
  sheet: number;
  at: CellRef;
  by?: string;
  reason?: string;
}

type Stored = "name" | "token" | "sheet" | "job";

/** 주소 경로(`/q4`)마다 이름·토큰·보던 시트·업무 id를 따로 기억한다. */
function storageKey(what: Stored) {
  return `mm.${storageSlug()}.${what}`;
}

function load(what: Stored): string | null {
  try {
    return localStorage.getItem(storageKey(what));
  } catch {
    return null;
  }
}

function save(what: Stored, value: string | null) {
  try {
    if (value === null) localStorage.removeItem(storageKey(what));
    else localStorage.setItem(storageKey(what), value);
  } catch {
    // 사생활 보호 모드 등: 기억하지 못해도 동작은 한다.
  }
}

const CLIENT_ID = Math.random().toString(36).slice(2, 10);

declare global {
  interface Window {
    /** E2E 전용: 연결을 끊고(다시 붙지 않음) 되살린다. */
    __mmTest?: { offline(on: boolean): void };
  }
}

/**
 * 같은 주소 경로를 다른 업무가 다시 쓰고 있는가(호스트가 앞 업무를 끝내고 새 업무에 같은 경로를 줌). 그러면 앞 업무에서
 * 보내지 못한 입력·토큰을 새 업무에 보내면 안 된다.
 */
export function isOtherJob(stored: string | null, job: string | null | undefined): boolean {
  return !!stored && !!job && stored !== job;
}

/** `slug`: 이 화면의 업무 주소 경로(`/q4` → `q4`). */
export function useSession(slug: string) {
  const [info, setInfo] = useState<SessionInfo | null>(null);
  /** 이 주소의 업무가 지금 공개되어 있지 않다(없는 주소·공개 중지·끝남). */
  const [missing, setMissing] = useState(false);
  const [phase, setPhase] = useState<Phase>(() => {
    const name = load("name");
    return name && load("token") ? { kind: "joining", name } : { kind: "name" };
  });
  const [welcome, setWelcome] = useState<Welcome | null>(null);
  /** 보고 있는 시트(세션 안 순번). 시트 이름을 주소 경로별로 기억한다. */
  const [active, setActiveState] = useState(0);
  const activeRef = useRef(0);
  activeRef.current = active;
  const [presence, setPresence] = useState<PresenceEntry[]>([]);
  const [connected, setConnected] = useState(false);
  /** 시트마다 내 필수 진행률. */
  const [progress, setProgress] = useState<PersonProgress[]>([]);
  const [readOnly, setReadOnly] = useState(false);
  /** 읽기 전용인 까닭(마감 시각이 지남 / 호스트가 마감함). */
  const [readOnlyReason, setReadOnlyReason] = useState<string | null>(null);
  const [history, setHistory] = useState<{ sheet: number; row: number; entries: ChangeEntry[] } | null>(null);
  const [notices, setNotices] = useState<Notice[]>([]);
  /** 다른 사람이 방금 바꾼 칸(`시트:행:열` → 바꾼 사람). FLASH_MS가 지나면 지운다. */
  const [recentEdits, setRecentEdits] = useState<ReadonlyMap<string, RecentEdit>>(new Map());
  /** 호스트가 보낸 공지·셀 강조·채팅. */
  const [announcements, setAnnouncements] = useState<NoticeView[]>([]);
  const [highlights, setHighlights] = useState<HighlightView[]>([]);
  const [chat, setChat] = useState<ChatMessage[]>([]);
  const [lockEvent, setLockEvent] = useState<LockEvent | null>(null);
  /** 표 아래에 행이 붙었다(내가 추가했으면 그 행으로 간다). */
  const [rowsAdded, setRowsAdded] = useState<{ seq: number; by: string; sheet: number; range: CellRange } | null>(null);
  /**
   * 호스트 PC 시계 − 이 기기 시계(ms). 마감까지 남은 시간은 호스트 시각으로 센다(`welcome.serverNow`·`pong.now`로
   * 맞춘다 — 기기 시계가 틀리거나 시간대가 달라도 호스트와 같게 보인다).
   */
  const [clockOffset, setClockOffset] = useState(0);
  const [, setRev] = useState(0);

  const ws = useRef<WebSocket | null>(null);
  const phaseRef = useRef(phase);
  phaseRef.current = phase;
  /** 호스트의 IP 명단에 있는 이 PC의 이름(`known`). */
  const knownName = useRef<string | null>(null);
  /** 연결이 열리면 보낼 입장 요청(재접속 포함). */
  const wantJoin = useRef<string | null>(phase.kind === "joining" ? phase.name : null);
  /** 끊긴 사유가 '다시 붙지 말 것'인지. */
  const stopRetry = useRef(false);
  const cursorTimer = useRef<number | undefined>(undefined);
  const pendingCursor = useRef<{ sheet: number; at: CellRef | null } | undefined>(undefined);
  /** 시트마다 편집 상태(불러온 순서). */
  const edits = useRef<EditState[]>([]);
  /** 입장 전에 읽어 둔 오프라인 대기열(이름별). */
  const saved = useRef(new Map<string, StoredBatch[]>());
  const saving = useRef<Promise<void>>(Promise.resolve());
  const noticeSeq = useRef(0);
  const flashSeq = useRef(0);
  const lockSeq = useRef(0);
  const rowsSeq = useRef(0);
  /** E2E: 오프라인 흉내(연결을 끊고 다시 붙지 않음). */
  const offline = useRef(false);
  const reconnect = useRef<() => void>(() => {});
  /** 지금 편집 상태가 누구 것인지(다른 이름으로 다시 들어오면 새로 만든다). */
  const welcomeName = useRef<string | null>(null);

  const send = useCallback((msg: ClientMsg): boolean => {
    if (ws.current?.readyState !== WebSocket.OPEN) return false;
    ws.current.send(JSON.stringify(msg));
    return true;
  }, []);

  const notify = useCallback((text: string, tone: Notice["tone"] = "info", action?: Notice["action"]) => {
    const id = ++noticeSeq.current;
    setNotices((list) => [...list.slice(-3), { id, text, tone, action }]);
  }, []);

  const dismiss = useCallback((id: number) => setNotices((list) => list.filter((n) => n.id !== id)), []);

  /** 다른 사람(호스트 포함)이 바꾼 칸을 잠깐 표시한다. 수식 결과(`by`가 빈 문자열)는 표시하지 않는다. */
  const flash = useCallback((sheet: number, by: string, cells: { row: number; col: number }[]) => {
    if (!by || cells.length === 0) return;
    const seq = ++flashSeq.current;
    const keys = cells.map((x) => `${sheet}:${x.row}:${x.col}`);
    setRecentEdits((m) => {
      const next = new Map(m);
      for (const k of keys) next.set(k, { by, seq });
      return next;
    });
    window.setTimeout(
      () =>
        setRecentEdits((m) => {
          const next = new Map(m);
          for (const k of keys) if (next.get(k)?.seq === seq) next.delete(k);
          return next;
        }),
      FLASH_MS,
    );
  }, []);

  const persist = useCallback(() => {
    const sheets = edits.current;
    const name = load("name");
    if (sheets.length === 0 || !name) return;
    const batches: StoredBatch[] = sheets.flatMap((st) =>
      st.outbox.map((b) => ({
        batch: b.batch,
        cells: b.cells,
        origin: b.origin,
        sheet: st.sheet,
        sheetName: st.model.sheet,
        layout: b.layout,
      })),
    );
    saving.current = saving.current.then(() => saveOutbox(outboxKey(name), batches));
  }, []);

  const refresh = useCallback(() => setRev((n) => n + 1), []);

  const sendJoin = useCallback(
    async (name: string) => {
      // 지난번에 보내지 못한 입력이 있으면 입장하기 전에 읽어 둔다(입장하면 바로 다시 보낸다).
      if (!saved.current.has(name)) saved.current.set(name, await loadOutbox(outboxKey(name)));
      send({
        type: "join",
        name,
        token: load("name") === name ? (load("token") ?? undefined) : undefined,
        protocol: PROTOCOL,
      });
    },
    [send],
  );

  const at = useCallback(
    (r: { row: number; col: number }, origin: CellRef) => a1(origin.row + r.row, origin.col + r.col),
    [],
  );

  /** 알림에 붙일 시트 이름(시트가 하나면 없음): `인력계획!`. */
  const place = (sheet: number): string => {
    const all = edits.current;
    return all.length > 1 && all[sheet] ? sheetPrefix(all[sheet].model.sheet) : "";
  };

  /** 시트 순번이 가리키는 편집 상태(없는 시트면 `undefined`). */
  const sheetOf = (sheet: number): EditState | undefined => edits.current[sheet];

  const onMessage = useCallback(
    (msg: ServerMsg) => {
      switch (msg.type) {
        case "welcome": {
          // 같은 주소를 새 업무가 쓰고 있으면 앞 업무의 대기열·편집 상태는 버린다.
          const otherJob = isOtherJob(load("job"), msg.session.job);
          if (msg.session.job) save("job", msg.session.job);
          if (otherJob) {
            saved.current.clear();
            edits.current = [];
            welcomeName.current = null;
            saving.current = saving.current.then(() => saveOutbox(outboxKey(msg.name), []));
          }
          save("name", msg.name);
          save("token", msg.token);
          wantJoin.current = msg.name; // 끊겼다 다시 붙으면 토큰으로 바로 입장
          const names = msg.sheets.map((v) => v.model.sheet);
          const same = welcomeName.current === msg.name;
          // 같은 사람이면 이름이 같은 시트의 편집 상태(대기열·되돌리기)를 이어 쓴다. 처음이면 저장해 둔 대기열을 나눠 싣는다.
          const before = same ? edits.current : [];
          const stored = same ? null : splitBySheet(saved.current.get(msg.name) ?? [], names);
          const next = msg.sheets.map((view, i) => {
            const old = before.find((e) => e.model.sheet === view.model.sheet);
            if (old) {
              old.sheet = i;
              for (const b of old.outbox) b.sheet = i;
              old.load(view);
              return old;
            }
            return new EditState(view, CLIENT_ID, i, stored?.[i] ?? []);
          });
          edits.current = next;
          welcomeName.current = msg.name;
          setClockOffset(msg.serverNow - Date.now());
          for (const st of next)
            for (const b of st.outbox)
              send({
                type: "setCells",
                sheet: b.sheet,
                batch: b.batch,
                cells: b.cells,
                origin: b.origin,
                layout: b.layout,
              });
          // 보던 시트를 이어서 본다(처음이면 지난번에 보던 시트). 그 시트가 없어졌으면 첫 시트.
          const keep = same ? before[activeRef.current]?.model.sheet : load("sheet");
          const shown = Math.max(0, names.indexOf(keep ?? ""));
          activeRef.current = shown;
          setActiveState(shown);
          setWelcome(msg);
          setPresence(msg.presence);
          setInfo(msg.session);
          setProgress(msg.sheets.map((v) => v.progress));
          setAnnouncements(msg.notices);
          setHighlights(msg.highlights);
          setChat(msg.chat);
          setReadOnly(msg.session.readOnly);
          setReadOnlyReason(
            msg.session.readOnly
              ? msg.session.closed
                ? "호스트가 취합을 마감했습니다"
                : "마감 시각이 지났습니다"
              : null,
          );
          setPhase({ kind: "in" });
          refresh();
          break;
        }
        case "known": {
          // 이 PC는 호스트의 IP 명단에 있다: 이름을 묻지 않고 그 이름으로 들어간다(이 연결의 입장 요청은 보낸 이름과
          // 상관없이 그 이름이 된다). 호스트가 연결을 끊었으면 '다시 참여'를 누를 때까지 기다린다.
          knownName.current = msg.name;
          if (phaseRef.current.kind === "kicked") break;
          const sent = wantJoin.current !== null; // 연결되자마자 기억한 이름으로 이미 보냈다
          wantJoin.current = msg.name;
          setPhase((p) => (p.kind === "in" ? p : { kind: "joining", name: msg.name, known: true }));
          if (!sent) void sendJoin(msg.name);
          break;
        }
        case "pending":
          setPhase({ kind: "pending", name: msg.name });
          break;
        case "dupName":
          setPhase({ kind: "dup", name: msg.name });
          break;
        case "rejected":
          wantJoin.current = null;
          save("token", null);
          setPhase({ kind: "name", error: msg.reason });
          break;
        case "kicked":
          wantJoin.current = null;
          stopRetry.current = msg.reason === "removed";
          if (msg.reason !== "switchedDevice") save("token", null);
          setPhase({ kind: "kicked", reason: msg.reason });
          break;
        case "presence":
          setPresence(msg.people);
          break;
        case "cursor": {
          const cursor: SheetCell | undefined = msg.at ? { sheet: msg.sheet, ...msg.at } : undefined;
          setPresence((list) => list.map((p) => (p.name === msg.name ? { ...p, cursor } : p)));
          break;
        }
        case "applied": {
          const st = sheetOf(msg.sheet);
          if (!st) break;
          const r = st.applied(msg.batch, msg.cells, msg.conflicts, msg.rejected);
          persist();
          refresh();
          const where = (x: { row: number; col: number }) => `${place(msg.sheet)}${at(x, st.model.origin)}`;
          if (r.conflicts.length > 0) {
            const first = where(r.conflicts[0]!);
            const more = r.conflicts.length > 1 ? ` 외 ${r.conflicts.length - 1}칸` : "";
            notify(`${first}${more}: 다른 사람이 먼저 바꾼 값을 내 입력으로 덮어썼습니다`, "warn");
          }
          if (r.rejected.length > 0) {
            const first = r.rejected[0]!;
            const more = r.rejected.length > 1 ? ` (외 ${r.rejected.length - 1}칸)` : "";
            notify(`${where(first)}: ${first.reason}${more}`, "error");
          }
          break;
        }
        case "cells":
          sheetOf(msg.sheet)?.cells(msg.cells);
          refresh();
          flash(msg.sheet, msg.by, msg.cells);
          break;
        case "lockResult":
          setLockEvent({
            seq: ++lockSeq.current,
            kind: msg.ok ? "granted" : "denied",
            sheet: msg.sheet,
            at: msg.at,
            by: msg.by,
            reason: msg.reason,
          });
          break;
        case "lockChanged":
          sheetOf(msg.sheet)?.lockChanged(msg.at, msg.by);
          refresh();
          if (!msg.by) setLockEvent({ seq: ++lockSeq.current, kind: "released", sheet: msg.sheet, at: msg.at });
          break;
        case "progress":
          setProgress((list) => list.map((p, i) => (i === msg.sheet ? msg.progress : p)));
          break;
        case "readOnly":
          setReadOnly(msg.readOnly);
          setReadOnlyReason(msg.readOnly ? (msg.reason ?? "마감되었습니다") : null);
          notify(
            msg.readOnly ? `${msg.reason ?? "마감되었습니다"}. 이제 입력할 수 없습니다` : "다시 입력할 수 있습니다",
            msg.readOnly ? "warn" : "info",
          );
          break;
        case "styles":
          sheetOf(msg.sheet)?.styles(msg.styles, msg.changes);
          refresh();
          break;
        case "history":
          setHistory({ sheet: msg.sheet, row: msg.row, entries: msg.entries });
          break;
        case "closing":
          if (msg.ended) {
            // 업무가 끝났다: 다시 붙지 않고, 남은 대기열·토큰도 쓸 일이 없다.
            stopRetry.current = true;
            wantJoin.current = null;
            save("token", null);
            const name = load("name");
            if (name) saving.current = saving.current.then(() => saveOutbox(outboxKey(name), []));
            setMissing(true);
            setPhase({ kind: "ended" });
          } else {
            const deadline = msg.deadline;
            setPhase((p) => (p.kind === "in" || p.kind === "pending" ? { kind: "closed", deadline } : p));
          }
          break;
        case "notice":
          setAnnouncements((list) => [...list.filter((n) => n.id !== msg.id), msg]);
          if (!msg.important) notify(`공지: ${msg.text}`, "info");
          break;
        case "highlight": {
          setHighlights((list) => [...list.filter((h) => h.id !== msg.id), msg]);
          const origin = sheetOf(msg.sheet)?.model.origin ?? { row: 0, col: 0 };
          const where = `${place(msg.sheet)}${a1(origin.row + msg.range.row, origin.col + msg.range.col)}`;
          notify(`${msg.from} 님이 ${where} 셀을 강조했습니다${msg.note ? `: ${msg.note}` : ""}`, "info", {
            label: "셀로 이동",
            range: msg.range,
            highlight: msg.id,
            sheet: msg.sheet,
          });
          break;
        }
        case "highlightCleared":
          setHighlights((list) => list.filter((h) => h.id !== msg.id));
          break;
        case "chat":
          setChat((list) => [...list, msg].slice(-500));
          break;
        case "chatReactions":
          setChat((list) => list.map((m) => (m.id === msg.id ? { ...m, reactions: msg.reactions } : m)));
          break;
        case "rowsAdded":
          setRowsAdded({ seq: ++rowsSeq.current, by: msg.by, sheet: msg.sheet, range: msg.range });
          break;
        case "rowsDeleted": {
          // 새 시트(welcome)는 먼저 왔다. 지운 사람이 내가 아니면 알린다(아래 행이 당겨졌다).
          const st = sheetOf(msg.sheet);
          if (st && msg.by !== welcomeName.current) {
            const first = st.model.origin.row + msg.from + 1;
            const rows = msg.count > 1 ? `${first}~${first + msg.count - 1}행` : `${first}행`;
            notify(`${msg.by} 님이 ${place(msg.sheet)}${rows}을 지웠습니다. 아래 행이 위로 당겨졌습니다`, "info");
          }
          break;
        }
        case "resized":
          sheetOf(msg.sheet)?.resized(msg.axis, msg.from, msg.count, msg.size);
          refresh();
          break;
        case "pong":
          setClockOffset(msg.now - Date.now());
          break;
        case "error":
          notify(msg.message, "error");
          break;
      }
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [at, flash, notify, persist, refresh, send, sendJoin],
  );

  // 연결 유지: 끊기면 점점 간격을 늘려 다시 붙고, 붙으면 기억한 이름(토큰)으로 다시 입장한다.
  useEffect(() => {
    let attempt = 0;
    let timer: number | undefined;
    let ping: number | undefined;
    let disposed = false;
    const connect = () => {
      if (offline.current) return;
      const scheme = location.protocol === "https:" ? "wss" : "ws";
      const socket = new WebSocket(`${scheme}://${location.host}/ws?s=${encodeURIComponent(slug)}`);
      ws.current = socket;
      socket.onopen = () => {
        attempt = 0;
        setConnected(true);
        setMissing(false); // 붙었으면 공개 중인 업무다(다시 공개됨)
        if (wantJoin.current) void sendJoin(wantJoin.current);
        ping = window.setInterval(() => send({ type: "ping" }), PING_MS);
      };
      socket.onmessage = (e) => onMessage(JSON.parse(e.data as string) as ServerMsg);
      socket.onclose = () => {
        setConnected(false);
        window.clearInterval(ping);
        if (ws.current === socket) ws.current = null;
        if (disposed || stopRetry.current || offline.current) return;
        timer = window.setTimeout(connect, RETRY_MS[Math.min(attempt++, RETRY_MS.length - 1)]);
      };
    };
    reconnect.current = () => {
      window.clearTimeout(timer);
      attempt = 0;
      connect();
    };
    connect();
    return () => {
      disposed = true;
      window.clearTimeout(timer);
      window.clearInterval(ping);
      ws.current?.close();
    };
  }, [onMessage, send, sendJoin, slug]);

  useEffect(() => {
    window.__mmTest = {
      offline(on: boolean) {
        offline.current = on;
        if (on) ws.current?.close();
        else reconnect.current();
      },
    };
    return () => {
      delete window.__mmTest;
    };
  }, []);

  // 접속 화면에 보일 공개 정보(접속자 수는 가끔 새로 읽는다). 404면 이 주소의 업무가 공개되어 있지 않다.
  useEffect(() => {
    if (phase.kind === "in" || phase.kind === "ended") return;
    const get = () =>
      fetch(`/api/session?s=${encodeURIComponent(slug)}`)
        .then(async (r) => {
          if (r.status === 404) {
            setMissing(true);
            return;
          }
          if (!r.ok) return;
          setInfo((await r.json()) as SessionInfo);
          setMissing(false);
        })
        .catch(() => {});
    get();
    const id = window.setInterval(get, 10_000);
    return () => window.clearInterval(id);
  }, [phase.kind, slug]);

  // 브라우저 탭에 업무명을 보인다(업무 여럿을 탭으로 열어 둘 때 가려 보기).
  const title = info?.title;
  useEffect(() => {
    document.title = title ? `${title} · 엑셀 취합` : "엑셀 취합";
  }, [title]);

  const join = useCallback(
    (name: string) => {
      const n = name.trim();
      if (!n) return;
      if (load("name") !== n) save("token", null);
      save("name", n);
      wantJoin.current = n;
      stopRetry.current = false;
      setPhase({ kind: "joining", name: n });
      void sendJoin(n);
    },
    [sendJoin],
  );

  const switchDevice = useCallback(() => {
    if (phase.kind !== "dup") return;
    wantJoin.current = phase.name;
    setPhase({ kind: "joining", name: phase.name });
    send({ type: "switchDevice", name: phase.name });
  }, [phase, send]);

  /** 이름 다시 입력(승인 대기·중복 접속 취소). */
  const reset = useCallback(() => {
    wantJoin.current = null;
    ws.current?.close(); // 서버의 대기 요청을 지우려면 연결을 새로 연다
    setPhase({ kind: "name" });
  }, []);

  /** 내 커서(지금 시트). 다른 사람에게 이름표로 보인다. */
  const cursor = useCallback(
    (at: CellRef | null) => {
      pendingCursor.current = { sheet: activeRef.current, at };
      if (cursorTimer.current !== undefined) return;
      cursorTimer.current = window.setTimeout(() => {
        cursorTimer.current = undefined;
        const c = pendingCursor.current;
        if (c) send({ type: "cursor", sheet: c.sheet, at: c.at ?? undefined });
      }, CURSOR_MS);
    },
    [send],
  );

  /** 묶음을 보낸다. 끊겨 있으면 대기열에 남아 다시 연결될 때 보낸다. */
  const ship = useCallback(
    (b: Batch | null) => {
      if (!b) return false;
      send({ type: "setCells", sheet: b.sheet, batch: b.batch, cells: b.cells, origin: b.origin, layout: b.layout });
      persist();
      refresh();
      return true;
    },
    [persist, refresh, send],
  );

  /** 지금 시트의 편집 상태. */
  const current = () => edits.current[activeRef.current] ?? null;

  /** 칸을 바꾼다(지금 시트, 값은 열 형식대로 정규화한 것). */
  const edit = useCallback(
    (changes: CellChange[], origin?: BatchOrigin) => ship(current()?.edit(changes, "undo", origin) ?? null),
    [ship],
  );
  /** 지금 시트에서 내 마지막 변경을 되돌린다(되돌리기는 시트마다). */
  const undo = useCallback(() => ship(current()?.undo() ?? null), [ship]);
  const redo = useCallback(() => ship(current()?.redo() ?? null), [ship]);
  /** 지금 시트의 구조 판(편집·잠금·서식에 실어 보낸다). */
  const layout = () => current()?.model.layout ?? 0;
  const lock = useCallback(
    (at: CellRef) => send({ type: "lock", sheet: activeRef.current, at, layout: layout() }),
    [send],
  );
  const unlock = useCallback((at: CellRef) => send({ type: "unlock", sheet: activeRef.current, at }), [send]);
  const rowHistory = useCallback((row: number) => send({ type: "rowHistory", sheet: activeRef.current, row }), [send]);
  const format = useCallback(
    (range: CellRange, patch: StylePatch) =>
      send({ type: "format", sheet: activeRef.current, range, patch, layout: layout() }),
    [send],
  );
  /** 보는 시트를 바꾼다(편집기는 먼저 닫아야 한다 — 확정·취소는 지금 시트로 간다). */
  const setActive = useCallback((sheet: number) => {
    const st = edits.current[sheet];
    if (!st || sheet === activeRef.current) return;
    activeRef.current = sheet;
    setActiveState(sheet);
    save("sheet", st.model.sheet);
  }, []);
  const ackNotice = useCallback(
    (id: number) => {
      setAnnouncements((list) => list.map((n) => (n.id === id ? { ...n, read: true } : n)));
      send({ type: "ackNotice", id });
    },
    [send],
  );
  const ackHighlight = useCallback(
    (id: number) => {
      setHighlights((list) => list.map((h) => (h.id === id ? { ...h, acked: true } : h)));
      send({ type: "ackHighlight", id });
    },
    [send],
  );
  const sendChat = useCallback((to: ChatTo, text: string) => send({ type: "chat", to, text }), [send]);
  /** 남의 채팅에 리액션을 달거나 뗀다(결과는 `chatReactions`로 온다). */
  const reactChat = useCallback((id: number, kind: ChatReaction) => send({ type: "chatReact", id, kind }), [send]);
  /** 지금 시트 표 아래에 빈 행을 붙인다(그 시트의 참여자 권한 '행 추가'). 결과는 새 시트(`welcome`) + `rowsAdded`. */
  const addRows = useCallback((count = 1) => send({ type: "addRows", sheet: activeRef.current, count }), [send]);
  /** 내가 추가한 행을 지운다(그 시트의 참여자 권한 '행 삭제'). 결과는 새 시트(`welcome`, 구조 판이 오른다) + `rowsDeleted`. */
  const deleteRows = useCallback(
    (from: number, count: number) =>
      send({ type: "deleteRows", sheet: activeRef.current, from, count, layout: layout() }),
    [send],
  );
  /** 행을 취소 표시하거나(`on`) 푼다(내가 입력할 수 있는 칸이 있는 행). 결과는 모두에게 새 시트. */
  const cancelRows = useCallback(
    (from: number, count: number, on: boolean) =>
      send({ type: "cancelRows", sheet: activeRef.current, from, count, on, layout: layout() }),
    [send],
  );
  /** 지금 시트의 열 너비·행 높이를 바꾼다(그 시트의 참여자 권한 '크기 조절'). 결과는 모두에게 `resized`. */
  const resize = useCallback(
    (axis: Axis, from: number, count: number, size: number) =>
      send({ type: "resize", sheet: activeRef.current, axis, from, count, size }),
    [send],
  );

  const view = welcome?.sheets[active] ?? null;
  const flags = useMemo(() => (view ? decodeFlags(view.flags) : new Uint8Array()), [view]);
  /** 모든 시트를 더한 내 필수 진행률. */
  const total = useMemo<PersonProgress | null>(
    () =>
      progress.length === 0
        ? null
        : progress.reduce((t, p) => ({
            name: p.name,
            rows: t.rows + p.rows,
            required: t.required + p.required,
            filled: t.filled + p.filled,
          })),
    [progress],
  );

  return {
    info,
    /** 이 주소의 업무가 지금 공개되어 있지 않다. */
    missing: missing && phase.kind !== "in",
    phase,
    welcome,
    /** 보고 있는 시트(세션 안 순번)와 그 시트의 보기(열 규칙·권한·행 담당자). */
    active,
    setActive,
    view,
    flags,
    presence,
    connected,
    /** 모든 시트를 더한 내 필수 진행률. */
    progress: total,
    /** 시트마다 내 필수 진행률. */
    sheetProgress: progress,
    readOnly,
    readOnlyReason,
    history,
    notices,
    announcements,
    highlights,
    chat,
    lockEvent,
    rowsAdded,
    /** 다른 사람이 방금 바꾼 칸(`시트:행:열`). */
    recentEdits,
    /** 지금 시트의 편집 상태. */
    sheet: edits.current[active] ?? null,
    /** 시트마다 편집 상태(불러온 순서). */
    sheets: edits.current,
    join,
    switchDevice,
    reset,
    cursor,
    edit,
    undo,
    redo,
    lock,
    unlock,
    rowHistory,
    format,
    ackNotice,
    ackHighlight,
    sendChat,
    reactChat,
    addRows,
    deleteRows,
    cancelRows,
    resize,
    /** 호스트 PC 시계 − 이 기기 시계(ms). */
    clockOffset,
    notify,
    dismiss,
    savedName: knownName.current ?? load("name"),
  };
}

export type SessionState = ReturnType<typeof useSession>;
