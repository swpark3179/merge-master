import { StrictMode } from "react";
import { createRoot } from "react-dom/client";

import { App } from "./App";
import "@mm/shared/tokens.css";
import "@mm/shared/motion.css";
import "./app.css";
import "./edit.css";

/** 글을 적는 칸이 아닌 입력(체크 상자·슬라이더 등). */
const NOT_TEXT = new Set(["checkbox", "radio", "range", "button", "submit", "reset", "color", "file", "image"]);

/** 글을 적는 칸(채팅·셀 내용·편집 중인 셀): 브라우저 메뉴로 붙여넣기 등을 쓰게 둔다. */
function writable(t: EventTarget | null): boolean {
  if (t instanceof HTMLTextAreaElement) return true;
  if (t instanceof HTMLInputElement) return !NOT_TEXT.has(t.type);
  return t instanceof HTMLElement && t.isContentEditable;
}

// 브라우저 기본 오른쪽 클릭 메뉴는 띄우지 않는다(그리드 메뉴가 없는 곳에서 헷갈려 잘못 누르지 않게).
document.addEventListener("contextmenu", (e) => {
  if (!writable(e.target)) e.preventDefault();
});

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
