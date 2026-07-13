# Review Queue Operational Runbook

Covers the human-review queue introduced by REQ-009 (IMP-REQ-009-13):
ambiguous project-mention matches flagged by the REQ-005 resolver, an
operator Confirm/Reject workflow, and an hourly SLA-overdue metric.

## Availability

The review queue routes are always registered and require HTTP Basic admin
authentication. REQ-005's resolver must be deployed for the queue to receive
ambiguous-match candidates.

## Routes (all require HTTP Basic admin auth, `middleware::admin_auth`)

| Route | Method | Purpose |
| --- | --- | --- |
| `/admin/review_candidates?status=open\|confirmed\|rejected` | GET | List candidates in a given status (defaults to `open`) |
| `/admin/review_candidates/{id}` | GET | Fetch one candidate's detail |
| `/admin/review_candidates/{id}/confirm` | POST | Body `{"version": N, "project_id": "<uuid>"}` — merges the candidate's mention into the given project |
| `/admin/review_candidates/{id}/reject` | POST | Body `{"version": N}` — marks the candidate rejected, leaves the mention unlinked |

Confirm/reject use optimistic concurrency: pass the `version` you last read
in the `GET` response. A stale `version` (someone else already
confirmed/rejected it, or the candidate moved on) returns `409 Conflict`
with no changes made — re-`GET` the candidate to see its current state
before retrying.

## SLA sweep (overdue metric)

`jobs::sla_sweep::compute_overdue_metric` reports how many `open`
candidates are past their `due_at` (2 business days from creation by
default — `domain::business_days::add_business_days`, weekday-only, no
Canadian statutory holiday calendar yet, see Known Limitations below).

This is a plain callable function, **not** a wired-up in-process scheduler —
this codebase has no periodic-execution infra anywhere (checked: no
`tokio::spawn`/interval loop in `main.rs`). Matching the same pattern
already established by REQ-001's `Scheduler::enqueue_due_fetches`, running
this "hourly" is a deployment-level concern: point an external cron job or
Kubernetes `CronJob` at a small binary/task that calls
`compute_overdue_metric` and forwards the result to `ALERT_WEBHOOK_URL`
(already configured in `.env.example`) or your metrics system. No such
external trigger exists yet in this repository — set one up before relying
on the overdue count in production.

## Manual re-sweep / re-check

There's no separate "reprocess" endpoint for the review queue (unlike
REQ-001/002's `/admin/*/reprocess`) — `compute_overdue_metric` is read-only
and can be invoked as often as needed with no side effects. To manually
check the current overdue count without waiting for the external
cron/CronJob:

```sql
SELECT count(*) FILTER (WHERE due_at < now()) AS overdue,
       count(*) AS open_total
FROM review_candidates
WHERE status = 'open';
```

## Known limitations

- **No Canadian statutory holiday calendar.** `add_business_days` is
  weekday-only (Mon–Fri); a `due_at` computed across e.g. Canada Day may be
  a day early relative to a true 2-*business*-day SLA. Documented, not
  blocking — revisit if the Founder reports SLA dates landing on holidays
  (see plan's Open Risks table, target 2026-08-15).
- **Single shared admin identity.** `actor` on `audit_events` is
  `ADMIN_USER` (one shared HTTP Basic account), not a per-operator session
  — there's no way to distinguish which specific person confirmed/rejected
  a given candidate from the audit trail alone.
- **No wired-up periodic execution** for the SLA sweep — see above.
