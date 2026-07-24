import { test, expect } from '@playwright/test';

// TC-014-6 (IMP-REQ-014-14): the project-detail page's non-modal "Get
// alerts — sign up" upsell CTA card (IMP-REQ-014-07) persists its
// collapsed/expanded state across page loads via `localStorage`, under the
// documented key `cta-upsell-collapsed`. `tests/cta_upsell.rs`'s
// `tc_014_6_cta_collapse_state_persistence_contract` already checks the
// STATIC contract (the rendered page ships the right script/key) from a
// pure HTTP response with no browser; this is the real cross-reload
// behavior that only a real browser can exercise, following the same
// pattern `filter-sheet.spec.ts`/`copy-link.spec.ts` already established
// for this app's other client-side-state requirements.
test('collapsing the CTA card persists across a page reload', async ({ page }) => {
  const projectId = process.env.SHOVELSUP_E2E_PROJECT_ID;
  test.skip(!projectId, 'global-setup did not seed a project id');

  await page.goto(`/projects/${projectId}`);

  const toggle = page.locator('#cta-collapse-toggle');
  const body = page.locator('#cta-upsell-body');

  await expect(toggle).toBeVisible();
  await expect(body).toBeVisible();
  await expect(toggle).toHaveAttribute('aria-expanded', 'true');

  await toggle.click();
  await expect(body).toBeHidden();
  await expect(toggle).toHaveAttribute('aria-expanded', 'false');

  await page.reload();

  // Still collapsed after the reload — the localStorage-backed contract.
  await expect(page.locator('#cta-upsell-body')).toBeHidden();
  await expect(page.locator('#cta-collapse-toggle')).toHaveAttribute('aria-expanded', 'false');

  // Non-modal, non-blocking (TC-014-2's precedent): the rest of the page is
  // still fully reachable while the card is collapsed.
  await expect(page.locator('#project-title')).toBeVisible();
  await expect(page.locator('#project-title')).toBeEnabled();
});

test('expanding the CTA card after a collapse also persists across a page reload', async ({
  page,
}) => {
  const projectId = process.env.SHOVELSUP_E2E_PROJECT_ID;
  test.skip(!projectId, 'global-setup did not seed a project id');

  await page.goto(`/projects/${projectId}`);

  const toggle = page.locator('#cta-collapse-toggle');
  const body = page.locator('#cta-upsell-body');

  await toggle.click();
  await expect(body).toBeHidden();

  await toggle.click();
  await expect(body).toBeVisible();
  await expect(toggle).toHaveAttribute('aria-expanded', 'true');

  await page.reload();

  await expect(page.locator('#cta-upsell-body')).toBeVisible();
  await expect(page.locator('#cta-collapse-toggle')).toHaveAttribute('aria-expanded', 'true');
});

test('the CTA signup link points at /signup in the same tab', async ({ page }) => {
  const projectId = process.env.SHOVELSUP_E2E_PROJECT_ID;
  test.skip(!projectId, 'global-setup did not seed a project id');

  await page.goto(`/projects/${projectId}`);

  const signupLink = page.locator('#cta-signup-link');
  await expect(signupLink).toBeVisible();
  await expect(signupLink).toHaveAttribute('href', /^\/signup/);
  await expect(signupLink).not.toHaveAttribute('target', '_blank');
});
