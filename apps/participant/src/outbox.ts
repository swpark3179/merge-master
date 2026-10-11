import type { Batch } from "./edits";
import { storageSlug } from "./slug";

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
  return `${storageSlug()}:${name}`;
}

/**
 * 저장하는 묶음: 시트는 이름으로도 남긴다(호스트가 시트를 다시 불러와 순서가 바뀌어도 맞는 시트에 다시 보낸다).
 * 여러 시트 전에 남긴 묶음에는 시트가 없다(첫 시트).
 */
export type StoredBatch = Omit<Batch, "sheet"> & { sheet?: number; sheetName?: string };

/** 저장해 둔 묶음을 지금 시트(이름 순서)마다 나눈다. 없어진 시트의 묶음은 버린다. */
export function splitBySheet(stored: StoredBatch[], names: string[]): Batch[][] {
  const out: Batch[][] = names.map(() => []);
  for (const b of stored) {
    const sheet = b.sheetName !== undefined ? names.indexOf(b.sheetName) : (b.sheet ?? 0);
    if (sheet < 0 || sheet >= names.length) continue;
    const { sheetName: _, ...rest } = b;
    out[sheet]!.push({ ...rest, sheet });
  }
  return out;
}

export async function loadOutbox(key: string): Promise<StoredBatch[]> {
  const d = await open();
  if (!d) return [];
  return new Promise((resolve) => {
    const req = d.transaction(STORE, "readonly").objectStore(STORE).get(key);
    req.onsuccess = () => resolve(Array.isArray(req.result) ? (req.result as StoredBatch[]) : []);
    req.onerror = () => resolve([]);
  });
}

export async function saveOutbox(key: string, batches: StoredBatch[]): Promise<void> {
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
