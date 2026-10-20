import type { FolderList } from "@mm/shared";
import { useEffect, useState } from "react";

import { appApi, errorText } from "../../api";

interface Props {
  /** 에이전트 주소(연결 확인한 것). */
  addr: string;
  /** 기본 저장 폴더(표시용). */
  baseDir: string;
  /** 처음 보일 폴더(기본 저장 폴더에서의 경로 조각). */
  initial: string[];
  onPick: (dir: string[]) => void;
  onClose: () => void;
}

/** 폴더·파일 이름으로 쓸 수 없는 이름이면 까닭(에이전트도 다시 검사한다). */
export function badName(name: string): string | null {
  const t = name;
  if (!t.trim() || t === "." || t === "..") return "이름을 입력해 주세요";
  if (/[<>:"/\\|?*\u0000-\u001f]/.test(t)) return '이름에 \\ / : * ? " < > | 는 쓸 수 없어요';
  if (/[. ]$/.test(t) || /^ /.test(t)) return "이름 앞뒤에 공백이나 끝에 점을 둘 수 없어요";
  if (/^(con|prn|aux|nul|com[1-9]|lpt[1-9])(\..*)?$/i.test(t.trim())) return "Windows가 쓰는 이름이라 쓸 수 없어요";
  if (t.length > 120) return "이름이 너무 길어요(120자까지)";
  return null;
}

/**
 * 에이전트 PC의 기본 저장 폴더 아래에서 마감 때 엑셀을 저장할 폴더를 고른다(이 PC의 파일 창은 에이전트 PC를 볼 수 없어
 * 에이전트에 물어 보인다). 없는 폴더(새 폴더)도 고를 수 있다 — 저장할 때 에이전트가 만든다.
 */
export function AgentFolderDialog({ addr, baseDir, initial, onPick, onClose }: Props) {
  const [path, setPath] = useState<string[]>(initial);
  const [list, setList] = useState<FolderList | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [newName, setNewName] = useState("");
  const nameProblem = newName ? badName(newName) : null;

  useEffect(() => {
    let alive = true;
    setLoading(true);
    setError(null);
    appApi
      .agentFolders(addr, path)
      .then((l) => alive && setList(l))
      .catch((e) => alive && setError(errorText(e)))
      .finally(() => alive && setLoading(false));
    return () => {
      alive = false;
    };
  }, [addr, path]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  const enter = (name: string) => setPath((p) => [...p, name]);
  const make = () => {
    if (!newName || nameProblem) return;
    enter(newName.trim());
    setNewName("");
  };

  return (
    <div className="modal-back" onClick={onClose}>
      <div
        className="modal card folder-dialog"
        role="dialog"
        aria-label="저장할 폴더 고르기"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="card-head">
          <span className="card-title">저장할 폴더 고르기</span>
          <span className="spacer" />
          <span className="muted small">에이전트 PC</span>
        </div>
        <nav className="crumbs" aria-label="지금 폴더">
          <button className="link" onClick={() => setPath([])} title={baseDir}>
            기본 저장 폴더
          </button>
          {path.map((seg, i) => (
            <span key={`${i}-${seg}`}>
              <span className="muted"> › </span>
              <button className="link" onClick={() => setPath(path.slice(0, i + 1))}>
                {seg}
              </button>
            </span>
          ))}
        </nav>
        <div className="muted small mono folder-path">{list?.display ?? baseDir}</div>
        <div className="folder-list" aria-busy={loading}>
          {list && !list.exists && <div className="muted small">아직 없는 폴더예요. 마감 때 저장하면서 만듭니다.</div>}
          {path.length > 0 && (
            <button className="folder-row" onClick={() => setPath(path.slice(0, -1))}>
              ‹ 위 폴더로
            </button>
          )}
          {list?.folders.map((f) => (
            <button key={f} className="folder-row" onClick={() => enter(f)}>
              <span className="folder-icon" aria-hidden /> {f}
            </button>
          ))}
          {list?.workbooks.map((f) => (
            <div key={f} className="folder-row file" title="이미 있는 엑셀 파일(같은 이름으로 저장하면 _2를 붙입니다)">
              {f}
            </div>
          ))}
          {list && list.exists && list.folders.length === 0 && list.workbooks.length === 0 && (
            <div className="muted small">빈 폴더예요.</div>
          )}
          {error && <div className="error small">{error}</div>}
        </div>
        <div className="row new-folder">
          <input
            aria-label="새 폴더 이름"
            placeholder="새 폴더 이름"
            value={newName}
            className={nameProblem ? "invalid" : undefined}
            onChange={(e) => setNewName(e.target.value)}
            onKeyDown={(e) => e.key === "Enter" && make()}
          />
          <button className="ghost" disabled={!newName || !!nameProblem} onClick={make}>
            새 폴더
          </button>
        </div>
        {nameProblem && <div className="warn small">{nameProblem}</div>}
        <div className="row">
          <span className="spacer" />
          <button className="ghost" onClick={onClose}>
            취소
          </button>
          <button onClick={() => onPick(path)} disabled={!!error}>
            이 폴더에 저장
          </button>
        </div>
      </div>
    </div>
  );
}
