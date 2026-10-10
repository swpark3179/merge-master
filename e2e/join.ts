import type { Page } from "@playwright/test";
import { expect } from "@playwright/test";

/**
 * 개발 서버의 참여자는 모두 127.0.0.1(호스트의 IP 명단 밖)에서 접속한다: 이름을 넣고 '참여하기'를 누르면 승인 대기가
 * 되고, 호스트 대신 관리 API(`/__dev/approve`)로 승인한다. `range`를 주면 그 범위를 배정하고 승인한다.
 */
export async function approve(page: Page, name: string, range?: string) {
  await expect(page.getByText("호스트 승인을 기다리는 중")).toBeVisible();
  const url = new URL("/__dev/approve", page.url()).href;
  expect((await page.request.post(url, { data: range ? { name, range } : { name } })).ok()).toBe(true);
}

/** 이름으로 참여 → 승인 → 작업 화면. */
export async function enter(page: Page, name: string, path = "/q4", range?: string) {
  await page.goto(path);
  await page.getByLabel("이름").fill(name);
  await page.getByRole("button", { name: "참여하기" }).click();
  await approve(page, name, range);
  await expect(page.locator(".status")).toContainText("실시간 동기화");
}

/** IP 명단을 바꾼다(이 PC = 127.0.0.1을 올리면 이름 없이 그 이름으로 바로 들어온다). */
export async function setIpNames(page: Page, names: Record<string, string>, origin = "http://127.0.0.1:8099") {
  const ok = await page.request.post(`${origin}/__dev/ip-names`, { data: JSON.stringify(names) });
  expect(ok.ok()).toBe(true);
}
