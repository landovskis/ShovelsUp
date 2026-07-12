use chrono::{TimeZone, Utc};
use shovelsup_pipeline::scheduler::Scheduler;
use sqlx::PgPool;

/// The 002 seed migration inserts Toronto/Vancouver/Montreal, so a fresh
/// migrated DB enqueues one job per seeded municipality.
#[sqlx::test(migrations = "../web/migrations")]
async fn test_enqueue_due_fetches_creates_one_job_per_municipality(pool: PgPool) {
    let now = Utc.with_ymd_and_hms(2026, 7, 10, 12, 0, 0).unwrap();
    let created = Scheduler::enqueue_due_fetches(&pool, now)
        .await
        .expect("enqueue should succeed");

    let seeded_count: i64 = sqlx::query_scalar!("SELECT count(*) FROM municipalities")
        .fetch_one(&pool)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(created.len() as i64, seeded_count);

    let job_count: i64 = sqlx::query_scalar!("SELECT count(*) FROM fetch_jobs")
        .fetch_one(&pool)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(job_count, seeded_count);
}

/// Running the scheduler twice on the same day must not create duplicate
/// jobs for a municipality that's already scheduled.
#[sqlx::test(migrations = "../web/migrations")]
async fn test_enqueue_due_fetches_is_idempotent_within_a_day(pool: PgPool) {
    let now = Utc.with_ymd_and_hms(2026, 7, 10, 9, 0, 0).unwrap();
    let later_same_day = Utc.with_ymd_and_hms(2026, 7, 10, 17, 0, 0).unwrap();

    let first_run = Scheduler::enqueue_due_fetches(&pool, now).await.unwrap();
    assert!(!first_run.is_empty(), "first run should enqueue jobs");

    let second_run = Scheduler::enqueue_due_fetches(&pool, later_same_day)
        .await
        .unwrap();
    assert!(
        second_run.is_empty(),
        "second run on the same day should not enqueue duplicates"
    );

    let job_count: i64 = sqlx::query_scalar!("SELECT count(*) FROM fetch_jobs")
        .fetch_one(&pool)
        .await
        .unwrap()
        .unwrap();
    let municipality_count: i64 = sqlx::query_scalar!("SELECT count(*) FROM municipalities")
        .fetch_one(&pool)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(job_count, municipality_count);
}

/// A new day is a new scheduling window — the daily fallback must fire again.
#[sqlx::test(migrations = "../web/migrations")]
async fn test_enqueue_due_fetches_fires_again_on_a_new_day(pool: PgPool) {
    let day_one = Utc.with_ymd_and_hms(2026, 7, 10, 23, 0, 0).unwrap();
    let day_two = Utc.with_ymd_and_hms(2026, 7, 11, 1, 0, 0).unwrap();

    let first_run = Scheduler::enqueue_due_fetches(&pool, day_one)
        .await
        .unwrap();
    let second_run = Scheduler::enqueue_due_fetches(&pool, day_two)
        .await
        .unwrap();

    assert!(!first_run.is_empty());
    assert_eq!(
        second_run.len(),
        first_run.len(),
        "crossing a day boundary should re-enqueue one job per municipality"
    );
}

/// With no municipalities present, the scheduler enqueues nothing and does
/// not error.
#[sqlx::test(migrations = "../web/migrations")]
async fn test_enqueue_due_fetches_with_no_municipalities(pool: PgPool) {
    // The 002 seed migration always runs; clear it to exercise the
    // zero-municipalities path explicitly.
    sqlx::query!("DELETE FROM municipalities")
        .execute(&pool)
        .await
        .unwrap();

    let now = Utc.with_ymd_and_hms(2026, 7, 10, 12, 0, 0).unwrap();
    let created = Scheduler::enqueue_due_fetches(&pool, now).await.unwrap();
    assert!(created.is_empty());
}
