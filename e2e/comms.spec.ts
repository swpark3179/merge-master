import type { Browser, BrowserContext, Page } from "@playwright/test";
import { expect, test } from "@playwright/test";

import { enter } from "./join";

// 6단계 참여자 쪽: 공지·셀 강조·채팅(1i), 셀 이력과 내 변경 되돌리기, 묶음 출처 기록.
// 호스트 동작은 개발 서버 관리 API(/__dev/*)로 대신한다.
// 열: F 신청 금액 · H 산출 근거. 행(0부터): 최도윤 7–11 · 김민지 12–16 · 정하늘 17–21

const F = 5;
const H = 7;

const contexts: BrowserContext[] = [];
test.afterEach(async () => {
  for (const c of contexts.splice(0)) await c.close();
});

async function open(browser: Browser, name: string): Promise<Page> {
  const context = await browser.newContext();
  contexts.push(context);
  const page = await context.newPage();
  await enter(page, name);
  return page;
}

const cell = (page: Page, r: number, c: number) => page.locator(`.mmg-cell[data-r="${r}"][data-c="${c}"]`);
const text = (page: Page, r: number, c: number) => cell(page, r, c).locator(".mmg-text");

test("중요 공지는 확인할 때까지 위에 고정되고 읽음이 집계된다", async ({ browser, request }) => {
  const a = await open(browser, "김민지");
  const ok = await request.post("/__dev/notice", {
    data: {
      text: "오늘 17시까지 산출 근거 입력 부탁드립니다.",
      important: true,
      target: { kind: "people", names: ["김민지"] },
    },
  });
  expect(ok.ok()).toBe(true);
  const bar = a.locator(".important-bar");
  await expect(bar).toContainText("오늘 17시까지 산출 근거 입력 부탁드립니다.");
  // 새로 고쳐도(다시 들어와도) 확인하기 전에는 남아 있다.
  await a.reload();
  await expect(bar).toBeVisible();
  await bar.getByRole("button", { name: "확인" }).click();
  await expect(bar).toHaveCount(0);
  const comms = await (await request.get("/__dev/comms")).json();
  const notice = comms.notices.find((n: { text: string }) => n.text.startsWith("오늘 17시까지"));
  expect(notice.read).toEqual(["김민지"]);

  // 일반 공지는 알림으로 뜨고 사이드바 '알림' 탭에 남는다(안 읽은 수는 사이드바 버튼에 보인다).
  await request.post("/__dev/notice", { data: { text: "참고: 양식 갱신", target: { kind: "all" } } });
  await expect(a.locator(".toast", { hasText: "공지: 참고: 양식 갱신" })).toBeVisible();
  await expect(a.getByRole("button", { name: /^사이드바/ }).locator(".badge")).toBeVisible();
  await a.locator(".side-panel").getByRole("tab", { name: /^알림/ }).click();
  await expect(a.locator(".side-panel")).toContainText("참고: 양식 갱신");
});

test("셀 강조: 노란 표시·말풍선과 '셀로 이동', 확인 집계, 해제", async ({ browser, request }) => {
  const a = await open(browser, "정하늘");
  const ok = await request.post("/__dev/highlight", {
    data: { range: "H20", note: "교육 과정명과 인원을 적어 주세요", audience: "owners" },
  });
  expect(ok.ok()).toBe(true);
  const toast = a.locator(".toast", { hasText: "H20 셀을 강조했습니다" });
  await expect(toast).toBeVisible();
  await expect(cell(a, 19, H)).toHaveClass(/deco-highlight/);
  await expect(cell(a, 19, H).locator(".callout")).toHaveText("교육 과정명과 인원을 적어 주세요");
  await toast.getByRole("button", { name: "셀로 이동" }).click();
  await expect(a.locator(".cell-detail .addr")).toHaveText("H20");
  await expect
    .poll(async () => {
      const c = await (await request.get("/__dev/comms")).json();
      return c.highlights.at(-1).acked.map((x: [string, number]) => x[0]);
    })
    .toEqual(["정하늘"]);
  const id = (await (await request.get("/__dev/comms")).json()).highlights.at(-1).id;
  await request.post("/__dev/highlight/clear", { data: { id } });
  await expect(cell(a, 19, H)).not.toHaveClass(/deco-highlight/);

  // 메모가 없으면 말풍선 없이 강조색만 보인다.
  await request.post("/__dev/highlight", { data: { range: "H21", note: "", audience: "owners" } });
  await expect(cell(a, 20, H)).toHaveClass(/deco-highlight/);
  await expect(a.locator(".callout")).toHaveCount(0);
  const plain = (await (await request.get("/__dev/comms")).json()).highlights.at(-1).id;
  await request.post("/__dev/highlight/clear", { data: { id: plain } });
  await expect(cell(a, 20, H)).not.toHaveClass(/deco-highlight/);
});

test("채팅: 전체·호스트에게, 호스트의 1:1 답장과 #셀 참조", async ({ browser, request }) => {
  const a = await open(browser, "최도윤");
  const b = await open(browser, "한지우");
  await a.locator(".side-panel").getByRole("tab", { name: /^채팅/ }).click();
  const input = a.getByLabel("채팅 입력");
  await input.fill("출장비 행 제가 실수로 지웠어요, 복구 가능할까요?");
  await input.press("Enter");
  await expect(a.getByLabel("채팅 내용")).toContainText("출장비 행 제가 실수로 지웠어요");
  // 다른 참여자에게도 보인다(패널을 열면 안 읽은 수가 사라진다).
  await expect(b.locator(".side-panel").getByRole("tab", { name: /^채팅\s*1/ })).toBeVisible();
  // 호스트(1c '내 이름': 박지훈)에게만
  await a.getByLabel("박지훈 님에게만").check();
  await input.fill("호스트님 질문");
  await input.press("Enter");
  await expect
    .poll(async () => (await (await request.get("/__dev/comms")).json()).chat.map((m: { text: string }) => m.text))
    .toContain("호스트님 질문");
  await b.locator(".side-panel").getByRole("tab", { name: /^채팅/ }).click();
  await expect(b.getByLabel("채팅 내용")).not.toContainText("호스트님 질문");

  await request.post("/__dev/chat", {
    data: { to: { kind: "person", name: "최도윤" }, text: "히스토리에서 복원했습니다 #F9" },
  });
  const reply = a.getByLabel("채팅 내용").locator(".chat-msg").last();
  // 호스트 글은 이름만 보인다('호스트'라고 붙이지 않는다).
  await expect(reply.locator("b")).toHaveText("박지훈");
  await reply.getByRole("button", { name: "#F9" }).click();
  await a.locator(".side-panel").getByRole("tab", { name: "셀 내용" }).click();
  await expect(a.locator(".cell-detail .addr")).toHaveText("F9");
  await expect(b.getByLabel("채팅 내용")).not.toContainText("히스토리에서 복원했습니다");
});

test("셀 이력(오른쪽 클릭)에서 내 마지막 변경을 되돌리고, 묶음 출처가 기록된다", async ({ browser, request }) => {
  const a = await open(browser, "정하늘");
  await cell(a, 20, F).click();
  await a.keyboard.type("1,200");
  await a.keyboard.press("Enter");
  await expect(text(a, 20, F)).toHaveText("1,200");
  await cell(a, 20, F).click({ button: "right" });
  const panel = a.locator(".side-panel");
  await expect(panel.getByRole("tab", { name: "셀 이력" })).toHaveAttribute("aria-selected", "true");
  await expect(panel).toContainText("나 입력");
  await panel.getByRole("button", { name: "되돌리기" }).click();
  await expect(text(a, 20, F)).toHaveText("");
  await expect(panel).toContainText("나 되돌림");
  const history = await (await request.get("/__dev/history")).json();
  const mine = history.filter(
    (e: { user: string; row: number; col: number }) => e.user === "정하늘" && e.row === 20 && e.col === F,
  );
  expect(mine.map((e: { origin: string; kind: string }) => `${e.origin}/${e.kind}`).slice(-2)).toEqual([
    "edit/input",
    "revert/revert",
  ]);

  // 붙여넣기·범위 지우기도 출처와 함께 남는다.
  await cell(a, 21, H).click();
  await a.evaluate(() => {
    const dt = new DataTransfer();
    dt.setData("text/plain", "붙여넣은 근거\r\n");
    document.activeElement!.dispatchEvent(
      new ClipboardEvent("paste", { clipboardData: dt, bubbles: true, cancelable: true }),
    );
  });
  await expect(text(a, 21, H)).toHaveText("붙여넣은 근거");
  await a.keyboard.press("Delete");
  await expect(text(a, 21, H)).toHaveText("");
  const all = await (await request.get("/__dev/history")).json();
  const h = all.filter((e: { row: number; col: number }) => e.row === 21 && e.col === H);
  expect(h.map((e: { origin: string }) => e.origin).slice(-2)).toEqual(["paste", "clear"]);
});
