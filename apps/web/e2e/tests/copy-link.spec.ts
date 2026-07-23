import { test, expect } from '@playwright/test';

// TC-013-3 (IMP-REQ-013-13): clicking the "Copy link" button (IMP-013-08
// markup + IMP-013-09 clipboard JS) writes the project's canonical URL to
// the clipboard and shows non-color-only UI feedback. Requires a real
// browser to exercise `navigator.clipboard` — the exact gap
// `tests/shareable_url.rs`'s `tc_013_3_...` documents (and leaves
// `#[ignore]`d, since it's a pure HTTP integration test with no browser).
// This is that test's real implementation, using the same headless-browser
// harness REQ-011 built (Playwright, not fantoccini — see
// `playwright.config.ts`'s doc comment for why).
//
// Clipboard read/write requires explicit permission grants in Chromium;
// `page.context().grantPermissions` (documented on Playwright's
// `BrowserContext` API) is the standard way to pre-authorize both
// `clipboard-read` and `clipboard-write` for this origin before the test
// interacts with the page, so the click handler's `navigator.clipboard.
// writeText` call succeeds exactly as it would for a real user who has
// granted the permission (or on a browser that doesn't prompt at all).
test('clicking Copy link writes the canonical URL to the clipboard and shows feedback', async ({
  page,
  context,
  baseURL,
}) => {
  const projectId = process.env.SHOVELSUP_E2E_PROJECT_ID;
  test.skip(!projectId, 'global-setup did not seed a project id');

  await context.grantPermissions(['clipboard-read', 'clipboard-write'], {
    origin: baseURL,
  });

  await page.goto(`/projects/${projectId}`);

  const copyButton = page.locator('#copy-link-button');
  await expect(copyButton).toBeVisible();
  await expect(copyButton).toHaveText('Copy link');

  await copyButton.click();

  const clipboardText = await page.evaluate(() => navigator.clipboard.readText());
  expect(clipboardText).toContain(`/projects/${projectId}`);
  expect(clipboardText.startsWith('https://')).toBe(true);

  // Non-color-only feedback (IMP-REQ-013-14): the button's own text swaps
  // to the "Copied!" label AND a separate `role="status"` element becomes
  // visible — either signal alone would be legible without relying on
  // color, but both are asserted here since both are part of the contract.
  await expect(copyButton).toHaveText('Copied!');
  const feedback = page.locator('#copy-link-feedback');
  await expect(feedback).toBeVisible();
});

test('French locale renders French Copy link label and feedback', async ({
  page,
  context,
  baseURL,
}) => {
  const projectId = process.env.SHOVELSUP_E2E_PROJECT_ID;
  test.skip(!projectId, 'global-setup did not seed a project id');

  await context.grantPermissions(['clipboard-read', 'clipboard-write'], {
    origin: baseURL,
  });

  await page.goto(`/projects/${projectId}?lang=fr`);

  const copyButton = page.locator('#copy-link-button');
  await expect(copyButton).toHaveText('Copier le lien');

  await copyButton.click();

  await expect(copyButton).toHaveText('Lien copié !');
  const feedback = page.locator('#copy-link-feedback');
  await expect(feedback).toBeVisible();
  await expect(feedback).toHaveText('Lien copié !');
});
