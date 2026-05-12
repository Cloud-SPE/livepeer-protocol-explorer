import { test, expect, type Page, type Route } from '@playwright/test';

const ROUTES: Array<{ name: string; hash: string }> = [
  { name: 'dashboard',          hash: '#/' },
  { name: 'orchestrators-list', hash: '#/orchestrators' },
  { name: 'gateways-list',      hash: '#/gateways' },
  { name: 'governance-list',    hash: '#/governance/proposals' },
  { name: 'reports-hub',        hash: '#/reports' },
  { name: 'rewards-leaderboard',hash: '#/rewards/leaderboard' },
  { name: 'performance-leaderboard', hash: '#/performance/leaderboard' },
];

const EMPTY_LIST = JSON.stringify({ data: [], meta: { chain_id: '42161' } });

async function mockApi(page: Page): Promise<void> {
  await page.route('**/*', async (route: Route) => {
    const url = route.request().url();
    const isApi = /\/(orchestrators|gateways|governance|reports|rewards|payouts|tickets|aggregations|valuations|events|prices|stake|transcoders|backfills|delegators|rounds|network)\b/.test(url);
    if (isApi) {
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: EMPTY_LIST,
      });
      return;
    }
    if (/\/(metrics|health)\b/.test(url)) {
      await route.fulfill({ status: 200, contentType: 'text/plain', body: '' });
      return;
    }
    await route.continue();
  });
}

test.describe('visual smoke', () => {
  for (const r of ROUTES) {
    test(r.name, async ({ page }) => {
      await mockApi(page);
      await page.goto(`/${r.hash}`);
      await page.waitForLoadState('networkidle');
      await expect(page).toHaveScreenshot(`${r.name}.png`, { fullPage: true });
    });
  }
});
