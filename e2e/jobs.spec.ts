import type { Browser, BrowserContext, Page } from "@playwright/test";
import { expect, test } from "@playwright/test";

import { approve } from "./join";

// 여러 업무 동시 진행: 개발 서버(포트 8096) 하나에 업무 두 개 — q4 = 예산신청서(--fixture budget), hr = 인력계획
// (`--session hr --fixture staff`). 예산 행(0부터): 최도윤 7–11 · 김민지 12–16. 인력 행: 김민지 3–4 · 정하늘 5–6 · 신예준 7.
// 열: 예산 F(5) 신청 금액 · 인력 E(4) 신규 요청. 서버 상태가 테스트 사이에 남으므로 테스트마다 다른 사람을 쓴다.

const ORIGIN = "http://127.0.0.1:8096";
const BUDGET = "2026년 부서별 예산 신청";
const STAFF = "2026 인력 계획 취합";

const contexts: BrowserContext[] = [];
test.afterEach(async () => {
  for (const c of contexts.splice(0)) await c.close();
});

async function context(browser: Browser): Promise<BrowserContext> {
  const c = await browser.newContext();
  contexts.push(c);
  return c;
}

/** 그 업무 주소로 이름을 넣고 참여 → (관리 API로) 승인 → 작업 화면. */
async function enterJob(page: Page, slug: string, name: string) {
  await page.goto(`${ORIGIN}/${slug}`);
  await page.getByLabel("이름").fill(name);
  await page.getByRole("button", { name: "참여하기" }).click();
  await approve(page, name);
  await expect(page.locator(".status")).toContainText("실시간 동기화");
}

const cell = (page: Page, r: number, c: number) => page.locator(`.mmg-cell[data-r="${r}"][data-c="${c}"]`);

async function typeInto(page: Page, r: number, c: number, value: string) {
  await cell(page, r, c).click();
  await page.keyboard.type(value);
  await page.keyboard.press("Enter");
}

type Entry = { sheet: number; row: number; col: number; user: string };

async function history(page: Page, slug: string): Promise<Entry[]> {
  return (await (await page.request.get(`${ORIGIN}/__dev/history?s=${slug}`)).json()) as Entry[];
}

const list = (page: Page) => page.getByRole("list", { name: "취합 업무 목록" });
const card = (page: Page, title: string) => list(page).getByRole("link", { name: new RegExp(title) });

test("맨 위 주소에는 공개 중인 업무가 업무명과 함께 보이고, 고르면 그 업무로 간다", async ({ page }) => {
  await page.goto(`${ORIGIN}/`);
  await expect(page.getByRole("heading", { name: "취합 업무" })).toBeVisible();
  await expect(list(page).getByRole("link")).toHaveCount(2);
  await expect(list(page).getByRole("link").nth(0)).toContainText(BUDGET);
  await expect(list(page).getByRole("link").nth(1)).toContainText(STAFF);
  await expect(card(page, STAFF)).toContainText("김팀장 님의 요청");
  await expect(card(page, BUDGET)).toContainText("박지훈 님의 요청");

  await card(page, STAFF).click();
  await expect(page).toHaveURL(`${ORIGIN}/hr`);
  await expect(page.getByRole("heading", { name: STAFF })).toBeVisible();
  await expect(page).toHaveTitle(`${STAFF} · 엑셀 취합`);
  await page.getByRole("link", { name: "← 다른 취합 업무 보기" }).click();
  await expect(page).toHaveURL(`${ORIGIN}/`);
});

test("같은 브라우저에서 두 업무에 따로 들어가 입력하고, 값·이름·토큰이 섞이지 않는다", async ({ browser }) => {
  const c = await context(browser);
  const q4 = await c.newPage();
  const hr = await c.newPage();
  await enterJob(q4, "q4", "최도윤");
  await enterJob(hr, "hr", "정하늘");
  await expect(q4.getByRole("heading", { name: BUDGET })).toBeVisible();
  await expect(hr.getByRole("heading", { name: STAFF })).toBeVisible();

  await typeInto(q4, 7, 5, "4321");
  await typeInto(hr, 5, 4, "2");
  await expect.poll(async () => (await history(q4, "q4")).some((e) => e.user === "최도윤" && e.row === 7)).toBe(true);
  await expect.poll(async () => (await history(hr, "hr")).some((e) => e.user === "정하늘" && e.row === 5)).toBe(true);
  expect((await history(q4, "q4")).some((e) => e.user === "정하늘")).toBe(false);
  expect((await history(hr, "hr")).some((e) => e.user === "최도윤")).toBe(false);

  // 이름·토큰은 업무 주소마다: 새로 고치면 그 업무에 그 이름으로 바로 들어온다.
  const stored = await q4.evaluate(() => [localStorage.getItem("mm.q4.name"), localStorage.getItem("mm.hr.name")]);
  expect(stored).toEqual(["최도윤", "정하늘"]);
  await hr.reload();
  await expect(hr.locator(".status")).toContainText("정하늘 님");
  await expect(hr.locator(".status")).toContainText("실시간 동기화");
  await expect(hr.getByRole("button", { name: "참여하기" })).toHaveCount(0);
  await q4.getByRole("link", { name: "업무 목록" }).click();
  await expect(q4).toHaveURL(`${ORIGIN}/`);
});

test("한 업무만 공개를 멈추면 그 업무 화면만 멈추고, 다시 공개하면 저절로 이어진다", async ({ browser }) => {
  const c = await context(browser);
  const q4 = await c.newPage();
  const hr = await c.newPage();
  await enterJob(q4, "q4", "김민지");
  await enterJob(hr, "hr", "신예준");
  expect((await hr.request.post(`${ORIGIN}/__dev/unpublish?s=hr`)).ok()).toBe(true);
  try {
    await expect(hr.getByText("서비스가 중지되었습니다")).toBeVisible();
    await expect(q4.locator(".status")).toContainText("실시간 동기화");
    await typeInto(q4, 12, 5, "777");
    await expect
      .poll(async () => (await history(q4, "q4")).some((e) => e.user === "김민지" && e.row === 12))
      .toBe(true);

    const other = await (await context(browser)).newPage();
    await other.goto(`${ORIGIN}/`);
    await expect(list(other).getByRole("link")).toHaveCount(1);
    await expect(card(other, BUDGET)).toBeVisible();
    await other.goto(`${ORIGIN}/hr`);
    await expect(other.getByText("이 업무는 지금 공개되어 있지 않습니다")).toBeVisible();
  } finally {
    expect((await hr.request.post(`${ORIGIN}/__dev/publish?s=hr`)).ok()).toBe(true);
  }
  // 다시 공개하면 기억한 토큰으로 다시 들어온다(승인 없이).
  await expect(hr.locator(".status")).toContainText("실시간 동기화", { timeout: 15_000 });
  await expect(hr.locator(".status")).toContainText("신예준 님");
});

test("호스트가 업무를 끝내면 그 업무 화면은 '끝났습니다'가 되고 다시 붙지 않는다", async ({ browser }) => {
  const c = await context(browser);
  const hr = await c.newPage();
  await enterJob(hr, "hr", "김민지");
  expect((await hr.request.post(`${ORIGIN}/__dev/unpublish?s=hr&ended=true`)).ok()).toBe(true);
  try {
    await expect(hr.getByText("호스트가 이 업무를 끝냈습니다")).toBeVisible();
    await expect(hr.getByRole("heading", { name: STAFF })).toBeVisible();
  } finally {
    expect((await hr.request.post(`${ORIGIN}/__dev/publish?s=hr`)).ok()).toBe(true);
  }
  await hr.waitForTimeout(1500);
  await expect(hr.getByText("호스트가 이 업무를 끝냈습니다")).toBeVisible();
  await expect(hr.evaluate(() => localStorage.getItem("mm.hr.token"))).resolves.toBeNull();
});

test("IP 명단에 있는 PC는 두 업무 모두 승인 없이 바로 들어가고, 목록에 '내 담당'이 붙는다", async ({ browser }) => {
  const c = await context(browser);
  const page = await c.newPage();
  const set = (names: Record<string, string>) =>
    page.request.post(`${ORIGIN}/__dev/ip-names`, { data: JSON.stringify(names) });
  expect((await set({ "127.0.0.1": "이서연" })).ok()).toBe(true);
  try {
    await page.goto(`${ORIGIN}/`);
    await expect(list(page).locator(".pill.mine")).toHaveCount(2);
    await card(page, STAFF).click();
    await expect(page.locator(".status")).toContainText("이서연 님");
    await expect(page.locator(".status")).toContainText("실시간 동기화");
    const q4 = await c.newPage();
    await q4.goto(`${ORIGIN}/q4`);
    await expect(q4.locator(".status")).toContainText("이서연 님");
    await expect(q4.getByRole("button", { name: "참여하기" })).toHaveCount(0);
  } finally {
    await set({});
  }
});

test("없는 업무 주소는 '공개되어 있지 않습니다'와 업무 목록 링크를 보인다", async ({ page }) => {
  await page.goto(`${ORIGIN}/nope`);
  await expect(page.getByText("이 업무는 지금 공개되어 있지 않습니다")).toBeVisible();
  await page.getByRole("link", { name: "공개 중인 업무 목록 보기" }).click();
  await expect(page).toHaveURL(`${ORIGIN}/`);
  await expect(list(page).getByRole("link")).toHaveCount(2);
});
