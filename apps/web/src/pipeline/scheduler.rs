use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

/// Enqueues one `fetch_jobs` row per municipality per day.
///
/// REQ-001's PRD calls for polling each municipality's meeting calendar and
/// fetching within hours of a scheduled meeting, falling back to a daily
/// cadence when no machine-readable calendar exists. No municipality in the
/// current seed data (`002_seed_municipalities.sql`) has a `calendar_url`, and
/// the PRD does not specify a calendar format to poll against — so this V1
/// always takes the daily-fallback path. Calendar-aware scheduling is a
/// documented gap, not a blocker (see IMPLEMENTATION_CHECKLIST.md risks).
pub struct Scheduler;

impl Scheduler {
    pub async fn enqueue_due_fetches(
        pool: &PgPool,
        now: DateTime<Utc>,
    ) -> Result<Vec<Uuid>, sqlx::Error> {
        let municipality_ids: Vec<Uuid> =
            sqlx::query_scalar!("SELECT id FROM municipalities").fetch_all(pool).await?;

        let mut created = Vec::new();
        for municipality_id in municipality_ids {
            let already_scheduled_today = sqlx::query_scalar!(
                "SELECT id FROM fetch_jobs \
                 WHERE municipality_id = $1 AND scheduled_for::date = $2::timestamptz::date",
                municipality_id,
                now
            )
            .fetch_optional(pool)
            .await?
            .is_some();

            if already_scheduled_today {
                continue;
            }

            let job_id = sqlx::query_scalar!(
                "INSERT INTO fetch_jobs (municipality_id, scheduled_for) \
                 VALUES ($1, $2) RETURNING id",
                municipality_id,
                now
            )
            .fetch_one(pool)
            .await?;
            created.push(job_id);
        }

        Ok(created)
    }
}
