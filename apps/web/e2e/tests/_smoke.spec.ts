import { test, expect } from '@playwright/test';

// IMP-REQ-011-13 harness bring-up: isolates "is Playwright itself broken"
// from "is my test/selector wrong" by driving a real headless Chromium
// instance against an inline `data:` URL (no network access required — this
// sandboxed environment has none, confirmed by a prior `https://example.com`
// attempt failing on DNS resolution, not on browser launch). Not part of the
// REQ-011 regression suite proper — kept only as a smoke check that the
// harness's browser automation (launch, navigate, DOM read, viewport
// resize) works at all.
test('harness smoke test: a real headless Chromium can navigate, read the DOM, and resize the viewport', async ({ page }) => {
  await page.setContent('<!doctype html><html><head><title>Harness smoke</title></head><body><h1>Harness smoke test</h1></body></html>');
  await expect(page).toHaveTitle('Harness smoke');
  const heading = page.locator('h1');
  await expect(heading).toHaveText('Harness smoke test');

  await page.setViewportSize({ width: 320, height: 640 });
  const viewportWidth = await page.evaluate(() => document.documentElement.clientWidth);
  expect(viewportWidth).toBe(320);
});
