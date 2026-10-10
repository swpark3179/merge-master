import type {
  BatchOrigin,
  CellRange,
  CellRef,
  ChangeEntry,
  ChatMessage,
  ChatTo,
  ClientMsg,
  HighlightView,
  KickReason,
  NoticeView,
  PersonProgress,
  PresenceEntry,
  ServerMsg,
  SessionInfo,
  StylePatch,
  Welcome,
} from "@mm/shared";
import { a1, decodeFlags } from "@mm/shared";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import type { Batch, CellChange } from "./edits";
import { EditState } from "./edits";
import { loadOutbox, outboxKey, saveOutbox } from "./outbox";

/** 서버 `protocol::PROTOCOL_VERSION`과 같아야 한다. */
const PROTOCOL = 4;
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
  /** 호스트가 서비스를 중지함(다시 열리면 자동으로 붙는다). */
  | { kind: "closed" };

export interface Notice {
  id: number;
  text: string;
  tone: "info" | "warn" | "error";
  /** 알림 안 버튼(예: 강조한 셀로 이동). */
  action?: { label: string; range: CellRange; highlight?: number };
}

/** 편집 잠금 알림: 내가 잡은 결과(받음·남이 쓰는 중), 또는 어떤 칸의 잠금이 풀렸다(남이 끝냄·호스트가 풂). */
export interface LockEvent {
  seq: number;
  kind: "granted" | "denied" | "released";
  at: CellRef;
  by?: string;
  reason?: string;
}

/** 주소 경로(`/q4`)마다 이름·토큰을 따로 기억한다. */
function storageKey(what: "name" | "token") {
  const slug = location.pathname.split("/").filter(Boolean)[0] ?? "root";
  return `mm.${slug}.${what}`;
}

function load(what: "name" | "token"): string | null {
  try {
    return localStorage.getItem(storageKey(what));
  } catch {
    return null;
  }
}

function save(what: "name" | "token", value: string | null) {
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

export function useSession() {
  const [info, setInfo] = useState<SessionInfo | null>(null);
  const [phase, setPhase] = useState<Phase>(() => {
    const name = load("name");
    return name && load("token") ? { kind: "joining", name } : { kind: "name" };
  });
  const [welcome, setWelcome] = useState<Welcome | null>(null);
  const [presence, setPresence] = useState<PresenceEntry[]>([]);
  const [connected, setConnected] = useState(false);
  const [progress, setProgress] = useState<PersonProgress | null>(null);
  const [readOnly, setReadOnly] = useState(false);
  /** 읽기 전용인 까닭(마감 시각이 지남 / 호스트가 마감함). */
  const [readOnlyReason, setReadOnlyReason] = useState<string | null>(null);
  const [history, setHistory] = useState<{ row: number; entries: ChangeEntry[] } | null>(null);
  const [notices, setNotices] = useState<Notice[]>([]);
  /** 호스트가 보낸 공지·셀 강조·채팅. */
  const [announcements, setAnnouncements] = useState<NoticeView[]>([]);
  const [highlights, setHighlights] = useState<HighlightView[]>([]);
  const [chat, setChat] = useState<ChatMessage[]>([]);
  const [lockEvent, setLockEvent] = useState<LockEvent | null>(null);
  /** 표 아래에 행이 붙었다(내가 추가했으면 그 행으로 간다). */
  const [rowsAdded, setRowsAdded] = useState<{ seq: number; by: string; range: CellRange } | null>(null);
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
  const pendingCursor = useRef<CellRef | null | undefined>(undefined);
  const edits = useRef<EditState | null>(null);
  /** 입장 전에 읽어 둔 오프라인 대기열(이름별). */
  const saved = useRef(new Map<string, Batch[]>());
  const saving = useRef<Promise<void>>(Promise.resolve());
  const noticeSeq = useRef(0);
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

  const persist = useCallback(() => {
    const st = edits.current;
    const name = load("name");
    if (!st || !name) return;
    const batches = st.outbox.map((b) => ({ batch: b.batch, cells: b.cells }));
    saving.current = saving.current.then(() => saveOutbox(outboxKey(name), batches));
  }, []);

  const refresh = useCallback(() => setRev(edits.current?.rev ?? 0), []);

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

  const onMessage = useCallback(
    (msg: ServerMsg) => {
      const st = edits.current;
      switch (msg.type) {
        case "welcome": {
          save("name", msg.name);
          save("token", msg.token);
          wantJoin.current = msg.name; // 끊겼다 다시 붙으면 토큰으로 바로 입장
          let state = st;
          if (state && welcomeName.current === msg.name) state.load(msg);
          else {
            state = new EditState(msg, CLIENT_ID, saved.current.get(msg.name) ?? []);
            edits.current = state;
          }
          welcomeName.current = msg.name;
          for (const b of state.outbox) send({ type: "setCells", batch: b.batch, cells: b.cells, origin: b.origin });
          setWelcome(msg);
          setPresence(msg.presence);
          setInfo(msg.session);
          setProgress(msg.progress);
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
        case "cursor":
          setPresence((list) => list.map((p) => (p.name === msg.name ? { ...p, cursor: msg.at } : p)));
          break;
        case "applied": {
          if (!st) break;
          const r = st.applied(msg.batch, msg.cells, msg.conflicts, msg.rejected);
          persist();
          refresh();
          const origin = st.model.origin;
          if (r.conflicts.length > 0) {
            const first = at(r.conflicts[0]!, origin);
            const more = r.conflicts.length > 1 ? ` 외 ${r.conflicts.length - 1}칸` : "";
            notify(`${first}${more}: 다른 사람이 먼저 바꾼 값을 내 입력으로 덮어썼습니다`, "warn");
          }
          if (r.rejected.length > 0) {
            const first = r.rejected[0]!;
            const more = r.rejected.length > 1 ? ` (외 ${r.rejected.length - 1}칸)` : "";
            notify(`${at(first, origin)}: ${first.reason}${more}`, "error");
          }
          break;
        }
        case "cells":
          st?.cells(msg.cells);
          refresh();
          break;
        case "lockResult":
          setLockEvent({
            seq: ++lockSeq.current,
            kind: msg.ok ? "granted" : "denied",
            at: msg.at,
            by: msg.by,
            reason: msg.reason,
          });
          break;
        case "lockChanged":
          st?.lockChanged(msg.at, msg.by);
          refresh();
          if (!msg.by) setLockEvent({ seq: ++lockSeq.current, kind: "released", at: msg.at });
          break;
        case "progress":
          setProgress(msg.progress);
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
          st?.styles(msg.styles, msg.changes);
          refresh();
          break;
        case "history":
          setHistory({ row: msg.row, entries: msg.entries });
          break;
        case "closing":
          setPhase((p) => (p.kind === "in" || p.kind === "pending" ? { kind: "closed" } : p));
          break;
        case "notice":
          setAnnouncements((list) => [...list.filter((n) => n.id !== msg.id), msg]);
          if (!msg.important) notify(`공지: ${msg.text}`, "info");
          break;
        case "highlight": {
          setHighlights((list) => [...list.filter((h) => h.id !== msg.id), msg]);
          const origin = st?.model.origin ?? { row: 0, col: 0 };
          const where = a1(origin.row + msg.range.row, origin.col + msg.range.col);
          notify(`${msg.from} 님이 ${where} 셀을 강조했습니다${msg.note ? `: ${msg.note}` : ""}`, "info", {
            label: "셀로 이동",
            range: msg.range,
            highlight: msg.id,
          });
          break;
        }
        case "highlightCleared":
          setHighlights((list) => list.filter((h) => h.id !== msg.id));
          break;
        case "chat":
          setChat((list) => [...list, msg].slice(-500));
          break;
        case "rowsAdded":
          setRowsAdded({ seq: ++rowsSeq.current, by: msg.by, range: msg.range });
          break;
        case "pong":
          break;
        case "error":
          notify(msg.message, "error");
          break;
      }
    },
    [at, notify, persist, refresh, send, sendJoin],
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
      const socket = new WebSocket(`${scheme}://${location.host}/ws`);
      ws.current = socket;
      socket.onopen = () => {
        attempt = 0;
        setConnected(true);
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
  }, [onMessage, send, sendJoin]);

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

  // 접속 화면에 보일 공개 정보(접속자 수는 가끔 새로 읽는다).
  useEffect(() => {
    if (phase.kind === "in") return;
    const get = () =>
      fetch("/api/session")
        .then((r) => r.json() as Promise<SessionInfo>)
        .then(setInfo)
        .catch(() => {});
    get();
    const id = window.setInterval(get, 10_000);
    return () => window.clearInterval(id);
  }, [phase.kind]);

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

  const cursor = useCallback(
    (at: CellRef | null) => {
      pendingCursor.current = at;
      if (cursorTimer.current !== undefined) return;
      cursorTimer.current = window.setTimeout(() => {
        cursorTimer.current = undefined;
        send({ type: "cursor", at: pendingCursor.current ?? undefined });
      }, CURSOR_MS);
    },
    [send],
  );

  /** 묶음을 보낸다. 끊겨 있으면 대기열에 남아 다시 연결될 때 보낸다. */
  const ship = useCallback(
    (b: Batch | null) => {
      if (!b) return false;
      send({ type: "setCells", batch: b.batch, cells: b.cells, origin: b.origin });
      persist();
      refresh();
      return true;
    },
    [persist, refresh, send],
  );

  /** 칸을 바꾼다(값은 열 형식대로 정규화한 것). */
  const edit = useCallback(
    (changes: CellChange[], origin?: BatchOrigin) => ship(edits.current?.edit(changes, "undo", origin) ?? null),
    [ship],
  );
  const undo = useCallback(() => ship(edits.current?.undo() ?? null), [ship]);
  const redo = useCallback(() => ship(edits.current?.redo() ?? null), [ship]);
  const lock = useCallback((at: CellRef) => send({ type: "lock", at }), [send]);
  const unlock = useCallback((at: CellRef) => send({ type: "unlock", at }), [send]);
  const rowHistory = useCallback((row: number) => send({ type: "rowHistory", row }), [send]);
  const format = useCallback((range: CellRange, patch: StylePatch) => send({ type: "format", range, patch }), [send]);
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
  /** 표 아래에 빈 행을 붙인다(참여자 권한 '행 추가'). 결과는 새 시트(`welcome`) + `rowsAdded`. */
  const addRows = useCallback((count = 1) => send({ type: "addRows", count }), [send]);

  const flags = useMemo(() => (welcome ? decodeFlags(welcome.flags) : new Uint8Array()), [welcome]);

  return {
    info,
    phase,
    welcome,
    flags,
    presence,
    connected,
    progress: progress ?? welcome?.progress ?? null,
    readOnly,
    readOnlyReason,
    history,
    notices,
    announcements,
    highlights,
    chat,
    lockEvent,
    rowsAdded,
    sheet: edits.current,
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
    addRows,
    notify,
    dismiss,
    savedName: knownName.current ?? load("name"),
  };
}

export type SessionState = ReturnType<typeof useSession>;
