# ADR 011 — Non-Modal Upsell CTA on the Project Detail Page

**Status**: Accepted
**Date**: 2026-07-22
**Feature**: Implementation Plan: Public Discovery & Search (IMP-REQ-014-05/-07/-08)

## Context

REQ-014 adds an upsell CTA ("Get alerts — sign up") to the project detail page, encouraging
an anonymous visitor to create an account for update notifications. Since every visitor to
this page is anonymous (REQ-010's own guarantee — these routes carry no authentication
mechanism at all, so there is no session state to branch on), the card is shown
unconditionally rather than gated on an "already signed up" check that this app has no way
to perform.

The open design question was how intrusive this upsell should be, given the app's own
constitution principle that no account is required for any search or view action
(REQ-010). A `<dialog>`/modal overlay — even a dismissible one — interrupts the primary
task (reading about a specific project) and, if scroll-locked, can make content
unreachable on a narrow viewport until dismissed.

Options considered:

| Option | Description |
|--------|-------------|
| Modal `<dialog>` overlay, scroll-locked until dismissed | Maximizes visibility/conversion pressure, but blocks the page's own content and conflicts with REQ-010's no-friction guarantee |
| Non-modal `<section>`, collapsible, no backdrop/scroll-lock | Visible but never blocking; the rest of the page stays fully interactive and readable at all times |
| Omit the CTA on first render, show only after a scroll/dwell trigger | Reduces immediate intrusiveness further, but adds complexity (scroll-position tracking) not asked for by the plan |

## Decision

Render the CTA as a plain `<section id="cta-upsell">` — never a `<dialog>`, never
`role="dialog"`, no backdrop element, no scroll-lock class (TC-014-2's exact, explicit
contract) — placed inline in `project_detail.html`'s existing flow, unconditionally for
every (anonymous) visitor.

- **Collapsible, not dismissible-forever**: an `id="cta-collapse-toggle"` button with
  `aria-expanded` toggles the card's visibility. State persists in `localStorage` under the
  key `cta-upsell-collapsed` for 30 days (IMP-REQ-014-09) — a visitor who collapses it once
  is not shown it again on every subsequent page load within that window, but it does
  reappear afterward rather than being silently gone forever.
- **Same-tab signup link**: `id="cta-signup-link"` has `href="/signup"` with no
  `target="_blank"` (TC-014-3) — following it is a normal same-tab navigation, not a
  pop-up/new-tab pattern that could feel like an ad.
- **Fully separate telemetry path**: the CTA's `impression`/`click` beacon
  (`POST /api/v1/cta-events`) is a distinct route/handler from the detail page's own
  rendering. A telemetry failure (DB down, rate-limited, origin-check rejection) must never
  affect the detail page's own response (TC-014-5) — the beacon is fire-and-forget from the
  client's perspective.
- **CSS is responsive but never `position: fixed`** (IMP-REQ-014-08): a fixed-position
  banner can visually cover page content on a narrow viewport regardless of scroll
  position, which is the same category of intrusiveness a modal has, just without the
  `<dialog>` semantics. The card occupies normal document flow at the existing 640px
  breakpoint convention.

## Rationale

- A modal was rejected primarily because it directly conflicts with REQ-010's own
  constitution principle: this app has already committed to zero friction for anonymous
  search/view actions, and a scroll-locking overlay reintroduces exactly the kind of
  friction that principle exists to prevent.
- Collapsible-with-persistence (rather than a one-time dismiss-forever) was chosen so the
  upsell isn't shown on literally every page view (annoying for a repeat visitor who has
  already said "not now"), while still surfacing again after enough time has passed that
  the visitor's circumstances may have changed — 30 days was the plan's own stated window,
  not derived from any measurement in this codebase.
- Making the telemetry endpoint failure-isolated from page rendering was a hard
  requirement (TC-014-5) independent of the modal-vs-non-modal decision, but reinforces the
  same "never let the upsell degrade the core product" principle this ADR is about.

## Consequences

- **No conversion-maximizing interstitial exists in this codebase**, by design. Any future
  requirement asking for a blocking/interstitial upsell pattern is a deliberate reversal of
  this decision and should get its own ADR explaining why the trade-off changed, not be
  added as an incremental tweak to this card.
- **`localStorage` is the only persistence mechanism for collapse state.** It is
  per-browser, not per-account (there are no accounts here) and not synced across devices;
  clearing site data resets the 30-day window.
- **The telemetry table (`cta_events`) has no read path in this pass** — it exists purely
  as a write sink for future analysis, with no dashboard or query built against it yet.
