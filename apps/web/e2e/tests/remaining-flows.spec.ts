import { test, expect } from '@playwright/test';

// IMP-REQ-011-16: remaining REQ-011 flow coverage not already exercised by
// no-horizontal-scroll.spec.ts / filter-sheet.spec.ts / accessibility.spec.ts.

test.use({ viewport: { width: 320, height: 640 } });

// IMP-REQ-011-11 (system-level half): the mobile-responsive markup —
// specifically the new filter-sheet trigger/close controls — must not break
// EN/FR localization. The Rust-side `search_labels_tests::
// en_and_fr_values_are_never_accidentally_identical` unit test already
// guards the label STRINGS never accidentally being byte-identical between
// languages; this test guards the actual rendered page, confirming the FR
// page really does render French label text through the responsive
// markup at a mobile viewport, not just that the label constants differ.
test('French search page renders French filter-sheet labels at 320px', async ({ page }) => {
  await page.goto('/search?lang=fr');
  await expect(page.locator('html')).toHaveAttribute('lang', 'fr');

  const trigger = page.locator('#filter-sheet-trigger');
  await expect(trigger).toHaveText('Filtres');

  await trigger.click();
  await expect(page.locator('.filter-sheet-title')).toHaveText('Filtres');
  await expect(page.locator('#filter-sheet-close')).toHaveAttribute(
    'aria-label',
    'Fermer les filtres',
  );
});

test('English search page renders English filter-sheet labels at 320px', async ({ page }) => {
  await page.goto('/search?lang=en');
  await expect(page.locator('html')).toHaveAttribute('lang', 'en');

  const trigger = page.locator('#filter-sheet-trigger');
  await expect(trigger).toHaveText('Filters');

  await trigger.click();
  await expect(page.locator('.filter-sheet-title')).toHaveText('Filters');
  await expect(page.locator('#filter-sheet-close')).toHaveAttribute(
    'aria-label',
    'Close filters',
  );
});

// IMP-REQ-011-05: empty/error state markup remains legible/usable at a
// mobile viewport (320px) — a no-results search and the fault-injected 503
// (IMP-REQ-011-08) both still carry the responsive shell and readable copy.
test('zero-results empty state is readable and shell-wrapped at 320px', async ({ page }) => {
  await page.goto('/search?q=no-such-project-should-ever-match-this-literal-string');
  await expect(page.locator('.search-empty')).toBeVisible();
  const { scrollWidth, clientWidth } = await page.evaluate(() => ({
    scrollWidth: document.documentElement.scrollWidth,
    clientWidth: document.documentElement.clientWidth,
  }));
  expect(scrollWidth).toBeLessThanOrEqual(clientWidth);
});

test('fault-injected 503 search error state is shell-wrapped and readable at 320px', async ({ page }) => {
  const response = await page.goto('/search?q=fault&force_fault=503');
  expect(response?.status()).toBe(503);
  await expect(page.locator('.search-error')).toBeVisible();
});

// IMP-REQ-011-06: the project-detail page's timeline/citation/description
// regions remain usable at a mobile viewport — a coarse smoke check
// (detailed layout assertions live in no-horizontal-scroll.spec.ts).
test('project detail page renders its core sections at 320px', async ({ page }) => {
  const projectId = process.env.SHOVELSUP_E2E_PROJECT_ID;
  test.skip(!projectId, 'global-setup did not seed a project id');
  await page.goto(`/projects/${projectId}`);

  await expect(page.locator('#project-title')).toBeVisible();
  await expect(page.locator('#project-timeline')).toBeVisible();
});
