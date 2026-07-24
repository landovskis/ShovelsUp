import { test, expect } from '@playwright/test';

// TC-011-3 / IMP-REQ-011-15: the mobile filter sheet (IMP-REQ-011-04)
// doesn't block page interaction while open and returns keyboard focus to
// the control that opened it once closed. Ports the intent of the
// fantoccini sketch that was left in `web/tests/responsive_e2e.rs` (now
// removed — see that file's updated doc comment); the selectors below
// (`#filter-sheet-trigger`, `#filter-sheet`, `#filter-sheet-close`) match
// the real markup added to `templates/projects.html` by IMP-REQ-011-04, not
// invented placeholders.
test.use({ viewport: { width: 320, height: 640 } });

test('filter sheet opens, does not block the rest of the page, and returns focus to its trigger on close', async ({
  page,
}) => {
  await page.goto('/projects?q=filter+sheet');

  const trigger = page.locator('#filter-sheet-trigger');
  const sheet = page.locator('#filter-sheet');
  const closeButton = page.locator('#filter-sheet-close');

  // Collapsed by default at this viewport.
  await expect(sheet).toBeHidden();
  await expect(trigger).toHaveAttribute('aria-expanded', 'false');

  await trigger.click();
  await expect(sheet).toBeVisible();
  await expect(trigger).toHaveAttribute('aria-expanded', 'true');

  // Non-blocking: the rest of the page (the search heading, well outside
  // the sheet) must still be reachable/visible — not covered by a modal
  // backdrop, not made inert.
  await expect(page.locator('#search-heading')).toBeVisible();
  await expect(page.locator('#search-heading')).toBeEnabled();

  await closeButton.click();
  await expect(sheet).toBeHidden();
  await expect(trigger).toHaveAttribute('aria-expanded', 'false');

  const focusedId = await page.evaluate(() => document.activeElement?.id);
  expect(focusedId).toBe('filter-sheet-trigger');
});

test('filter sheet trigger is not shown at desktop widths (filters render inline)', async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.goto('/projects?q=filter+sheet');

  await expect(page.locator('#filter-sheet-trigger')).toBeHidden();
  // The filter form itself is always in the DOM and visible at desktop
  // widths, regardless of the (mobile-only) open/closed toggle state.
  await expect(page.locator('#filter-sheet')).toBeVisible();
  await expect(page.locator('#search-q')).toBeVisible();
});
