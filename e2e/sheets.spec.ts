import type { Browser, BrowserContext, Page } from "@playwright/test";
import { expect, test } from "@playwright/test";

import { enter } from "./join";

// 여러 시트 취합: 개발 서버(포트 8097)에 예산신청서 + 인력계획(--fixture 두 번, --required F --required 2:E).
// 인력계획 행(0부터): 머리글 0 · 이서연 1–2 · 김민지 3–4 · 정하늘 5–6 · 신예준 7 · 담당자 없음(공통) 8
// 열: B 담당자(행 배정 기준) · E 신규 요청(숫자, 필수) · F 사유(글자)
// 서버 상태가 테스트 사이에 남으므로 테스트마다 다른 사람(행)을 쓴다.

const ORIGIN = "http://127.0.0.1:8097";
const E = 4;
const F = 5;

const contexts: BrowserContext[] = [];
test.afterEach(async () => {
  for (const c of contexts.splice(0)) await c.close();
});

async function open(browser: Browser, name: string): Promise<Page> {
  const context = await browser.newContext();
  contexts.push(context);
  const page = await context.newPage();
  await enter(page, name, `${ORIGIN}/q4`);
  return page;
}

const tabs = (page: Page) => page.getByRole("tablist", { name: "시트" });
const tab = (page: Page, name: string) => tabs(page).getByRole("tab", { name: new RegExp(`^${name}`) });
const cell = (page: Page, r: number, c: number) => page.locator(`.mmg-cell[data-r="${r}"][data-c="${c}"]`);
const text = (page: Page, r: number, c: number) => cell(page, r, c).locator(".mmg-text");

async function typeInto(page: Page, r: number, c: number, value: string) {
  await cell(page, r, c).click();
  await page.keyboard.type(value);
  await page.keyboard.press("Enter");
}

/** 브라우저(IndexedDB)에 저장된 대기열의 칸 수. 저장은 비동기라, 입력 직후(수 ms 안에) 새로 고치면 아직 없을 수 있다. */
async function storedCells(page: Page): Promise<number> {
  return page.evaluate(
    () =>
      new Promise<number>((resolve) => {
        const req = indexedDB.open("merge-master");
        req.onerror = () => resolve(0);
        req.onsuccess = () => {
          const all = req.result.transaction("outbox", "readonly").objectStore("outbox").getAll();
          all.onerror = () => resolve(0);
          all.onsuccess = () =>
            resolve((all.result as { cells: unknown[] }[][]).flat().reduce((n, b) => n + b.cells.length, 0));
        };
      }),
  );
}

async function history(page: Page) {
  return (await (await page.request.get(`${ORIGIN}/__dev/history`)).json()) as {
    sheet: number;
    row: number;
    col: number;
    user: string;
  }[];
}

test("시트가 하나면 시트 탭이 없다", async ({ browser }) => {
  const context = await browser.newContext();
  contexts.push(context);
  const page = await context.newPage();
  await enter(page, "한지우");
  await expect(page.locator(".sheet-tabs")).toHaveCount(0);
});

test("그리드 아래 시트 탭으로 오가며 입력하고, 다른 사람 화면의 그 시트에 보인다", async ({ browser }) => {
  const a = await open(browser, "김민지");
  const b = await open(browser, "이서연");
  await expect(tabs(a).getByRole("tab")).toHaveText([/^예산신청서/, /^인력계획2$/]);
  await expect(tab(a, "예산신청서")).toHaveAttribute("aria-selected", "true");

  await tab(a, "인력계획").click();
  await expect(tab(a, "인력계획")).toHaveAttribute("aria-selected", "true");
  await expect(text(a, 3, 1)).toHaveText("김민지");
  await typeInto(a, 3, E, "3");
  await expect(text(a, 3, E)).toHaveText("3");
  await expect(tab(a, "인력계획").locator(".left")).toHaveText("1"); // 내 필수 미입력이 하나 줄었다
  await expect.poll(async () => (await history(a)).at(-1)).toMatchObject({ sheet: 1, row: 3, col: E, user: "김민지" });

  // 다른 사람은 그 시트로 가면 바로 보인다. 같은 주소의 예산신청서 칸은 그대로다.
  const budgetE4 = await text(b, 3, E).textContent();
  await tab(b, "인력계획").click();
  await expect(text(b, 3, E)).toHaveText("3");
  await tab(b, "예산신청서").click();
  await expect(text(b, 3, E)).toHaveText(budgetE4 ?? "");

  // 되돌리기는 시트마다: 인력계획에서 Ctrl+Z
  await cell(a, 4, F).click();
  await a.keyboard.press("Control+z");
  await expect(text(a, 3, E)).toHaveText("");
  // 새로 고쳐도 보던 시트(인력계획)를 이어서 본다.
  await a.reload();
  await expect(a.locator(".status")).toContainText("실시간 동기화");
  await expect(tab(a, "인력계획")).toHaveAttribute("aria-selected", "true");
  await expect(text(a, 3, 1)).toHaveText("김민지");
});

test("담당자 없는 칸은 어느 시트든 명단의 누구나 입력한다", async ({ browser }) => {
  const a = await open(browser, "한지우"); // 인력계획에는 맡은 행이 없다
  await tab(a, "인력계획").click();
  await typeInto(a, 8, F, "공통 증원");
  await expect(text(a, 8, F)).toHaveText("공통 증원");
  await cell(a, 3, F).click();
  await a.keyboard.type("x");
  await expect(a.locator(".toast", { hasText: "이 칸은 입력할 수 없습니다" })).toBeVisible();
});

test("다른 시트의 강조와 채팅 #시트!칸 참조는 그 시트로 데려간다", async ({ browser, request }) => {
  const a = await open(browser, "정하늘");
  await expect(tab(a, "예산신청서")).toHaveAttribute("aria-selected", "true");
  const ok = await request.post(`${ORIGIN}/__dev/highlight`, {
    data: { range: "인력계획!E6", note: "증원 인원을 적어 주세요", audience: "owners" },
  });
  expect(ok.ok()).toBe(true);
  await expect(tab(a, "인력계획").locator(".dot")).toBeVisible();
  const toast = a.locator(".toast", { hasText: "인력계획!E6 셀을 강조했습니다" });
  await toast.getByRole("button", { name: "셀로 이동" }).click();
  await expect(tab(a, "인력계획")).toHaveAttribute("aria-selected", "true");
  await expect(cell(a, 5, E)).toHaveClass(/deco-highlight/);
  await expect(tab(a, "인력계획").locator(".dot")).toHaveCount(0); // 이동하면 확인으로

  await tab(a, "예산신청서").click();
  await request.post(`${ORIGIN}/__dev/chat`, { data: { to: { kind: "all" }, text: "#인력계획!F7 사유도 부탁해요" } });
  const side = a.locator(".side-panel");
  if (!(await side.isVisible())) await a.getByRole("button", { name: /^사이드바/ }).click();
  await side.getByRole("tab", { name: /^채팅/ }).click();
  await side.getByRole("button", { name: "인력계획!F7" }).click();
  await expect(tab(a, "인력계획")).toHaveAttribute("aria-selected", "true");
});

test("끊긴 동안 다른 시트에 한 입력도 새로 고친 뒤 그 시트에 보낸다", async ({ browser }) => {
  const a = await open(browser, "신예준");
  await tab(a, "인력계획").click();
  await a.evaluate(() => window.__mmTest!.offline(true));
  await expect(a.locator(".status")).toContainText("오프라인");
  await typeInto(a, 7, E, "4");
  await expect(a.locator(".status")).toContainText("입력 1건이 대기 중");
  // 사람이 새로 고치는 것보다 테스트가 훨씬 빠르다: 대기열이 저장된 뒤에 새로 고친다.
  await expect.poll(() => storedCells(a)).toBe(1);
  await a.reload();
  await expect(a.locator(".status")).toContainText("실시간 동기화");
  await expect.poll(async () => (await history(a)).at(-1)).toMatchObject({ sheet: 1, row: 7, col: E, user: "신예준" });
  await expect(a.locator(".status")).not.toContainText("대기 중");
});
