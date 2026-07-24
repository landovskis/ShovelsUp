import { test, expect } from '@playwright/test';

// TC-011-2 / IMP-REQ-011-14: at a 320px viewport, no element on any of the
// search/results/detail pages causes horizontal scroll
// (`document.documentElement.scrollWidth <= document.documentElement.clientWidth`).
// Ports the intent of the fantoccini sketch that was left in
// `web/tests/responsive_e2e.rs` (now removed — see that file's updated doc
// comment) to real Playwright assertions against a real headless Chromium
// instance, since accurate layout measurement needs a real browser, not
// `tower::ServiceExt::oneshot`.
test.use({ viewport: { width: 320, height: 640 } });

async function assertNoHorizontalScroll(page: import('@playwright/test').Page, url: string) {
  await page.goto(url);
  const { scrollWidth, clientWidth } = await page.evaluate(() => ({
    scrollWidth: document.documentElement.scrollWidth,
    clientWidth: document.documentElement.clientWidth,
  }));
  expect(
    scrollWidth,
    `${url} must not cause horizontal scroll at 320px (scrollWidth ${scrollWidth} > clientWidth ${clientWidth})`,
  ).toBeLessThanOrEqual(clientWidth);
}

test('home page has no horizontal scroll at 320px', async ({ page }) => {
  await assertNoHorizontalScroll(page, '/');
});

test('search page (no query) has no horizontal scroll at 320px', async ({ page }) => {
  await assertNoHorizontalScroll(page, '/projects');
});

test('search results page has no horizontal scroll at 320px', async ({ page }) => {
  await assertNoHorizontalScroll(page, '/projects?q=e2e+harness');
});

test('project detail page has no horizontal scroll at 320px', async ({ page }) => {
  const projectId = process.env.SHOVELSUP_E2E_PROJECT_ID;
  test.skip(!projectId, 'global-setup did not seed a project id');
  await assertNoHorizontalScroll(page, `/projects/${projectId}`);
});

// IMP-REQ-011-07 regression: the 404/400 error pages now route through the
// same responsive shell as every other page (TC-011-4), so they must be
// just as free of horizontal overflow at 320px as a normal page.
test('project not-found (404) error page has no horizontal scroll at 320px', async ({ page }) => {
  await assertNoHorizontalScroll(page, '/projects/00000000-0000-0000-0000-000000000000');
});

test('project malformed-id (400) error page has no horizontal scroll at 320px', async ({ page }) => {
  await assertNoHorizontalScroll(page, '/projects/not-a-uuid');
});
