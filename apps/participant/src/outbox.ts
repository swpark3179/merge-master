import type { Batch } from "./edits";

/**
 * 보내지 못한 입력(오프라인 대기열)을 IndexedDB에 둔다. 연결이 끊긴 채 창을 닫거나 새로 고쳐도 남아,
 * 다시 들어오면 보낸다. IndexedDB를 쓸 수 없으면(사생활 보호 모드 등) 메모리에만 둔다.
 */

const DB = "merge-master";
const STORE = "outbox";

let db: Promise<IDBDatabase | null> | null = null;

function open(): Promise<IDBDatabase | null> {
  db ??= new Promise((resolve) => {
    try {
      const req = indexedDB.open(DB, 1);
      req.onupgradeneeded = () => req.result.createObjectStore(STORE);
      req.onsuccess = () => resolve(req.result);
      req.onerror = () => resolve(null);
    } catch {
      resolve(null);
    }
  });
  return db;
}

/** 주소 경로(`/q4`)와 이름마다 따로 둔다. */
export function outboxKey(name: string): string {
  const slug = location.pathname.split("/").filter(Boolean)[0] ?? "root";
  return `${slug}:${name}`;
}

export async function loadOutbox(key: string): Promise<Batch[]> {
  const d = await open();
  if (!d) return [];
  return new Promise((resolve) => {
    const req = d.transaction(STORE, "readonly").objectStore(STORE).get(key);
    req.onsuccess = () => resolve(Array.isArray(req.result) ? (req.result as Batch[]) : []);
    req.onerror = () => resolve([]);
  });
}

export async function saveOutbox(key: string, batches: Batch[]): Promise<void> {
  const d = await open();
  if (!d) return;
  await new Promise<void>((resolve) => {
    const tx = d.transaction(STORE, "readwrite");
    const store = tx.objectStore(STORE);
    if (batches.length === 0) store.delete(key);
    else store.put(batches, key);
    tx.oncomplete = () => resolve();
    tx.onerror = () => resolve();
  });
}
