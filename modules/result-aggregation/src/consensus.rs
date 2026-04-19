//! Consensus policy hand-off for the task-distribution module.
//!
//! `task-distribution::submit_task` calls [`try_resolve_consensus`] once a
//! task has accumulated at least `redundancy_target` Submitted assignments.
//! This function runs *inside* the caller's transaction — it does NOT
//! commit — and applies the decision table from the spec:
//!
//! | Submissions | Distinct hashes           | Outcome     |
//! |-------------|---------------------------|-------------|
//! | 2           | 1                         | Completed   |
//! | 2           | 2                         | Escalated   |
//! | 3           | ≤2 with a ≥2-match hash   | Completed   |
//! | 3           | 3                         | Failed      |
//! | >3 defensive| ≥2-match hash present     | Completed   |
//!
//! The caller owns job-terminality recompute; we only touch
//! `tasks.status`, `tasks.winning_assignment_id`, and
//! `tasks.redundancy_target`.

use sqlx::{Postgres, Transaction};
use uuid::Uuid;

/// Possible outcomes of a consensus attempt.
///
/// `Completed` and `Failed` are terminal for the task. `Escalated` means
/// the caller should NOT flip task status — we simply bumped
/// `redundancy_target` so `pick_next_task` will dispatch to one more
/// worker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConsensusOutcome {
    /// A majority (or the only group) of hashes agreed; task marked
    /// `completed` and `winning_assignment_id` set.
    Completed,
    /// Exactly two disagreeing submissions; bumped `redundancy_target`
    /// from 2 to 3 and left the task in `awaiting_consensus`.
    Escalated,
    /// ≥3 submissions and every hash is unique; task marked `failed`.
    Failed,
}

#[derive(sqlx::FromRow)]
struct HashGroup {
    /// The distinct `result_hash` value this row groups. Used to run a
    /// second query for an arbitrary assignment_id from the winning group
    /// (Postgres `MIN()` does not work on `uuid` columns without an
    /// extension, so we pick a member via a separate LIMIT 1 query).
    result_hash: Option<String>,
    count: i64,
}

/// Resolve consensus for `task_id` inside the caller's transaction.
///
/// Side effects (all inside `tx`, no commit):
/// 1. `SELECT ... FOR UPDATE` on `tasks.task_id` to serialize with any
///    concurrent writer.
/// 2. Read every `Submitted` assignment's `result_hash`, group & count.
/// 3. Apply the decision table.
/// 4. Write task status / winning_assignment_id / redundancy_target.
///
/// Returns the outcome the caller uses to decide whether a job-terminality
/// recompute is required.
pub async fn try_resolve_consensus(
    tx: &mut Transaction<'_, Postgres>,
    task_id: Uuid,
) -> Result<ConsensusOutcome, sqlx::Error> {
    // 1. Lock the task row. This serializes us with the picker and any
    //    other `submit_task` racing on the same task.
    sqlx::query_scalar::<_, Uuid>(
        r#"SELECT task_id FROM tasks WHERE task_id = $1 FOR UPDATE"#,
    )
    .bind(task_id)
    .fetch_one(&mut **tx)
    .await?;

    // 2. Group Submitted assignments by result_hash.
    //
    //    NOTE: `result_hash` is nullable at the column level, but any row
    //    whose `status = 'submitted'` has been written by `submit_task`
    //    which always populates the hash. We still tolerate nullable in
    //    the type below so a malformed row doesn't poison the planner.
    //    The `AND result_hash IS NOT NULL` filter is defense-in-depth: it
    //    prevents a malformed row from forming a bogus NULL-majority group
    //    (SQL `GROUP BY` treats NULLs as a single group) that would then be
    //    treated as a real hash by the decision table below.
    //
    //    We do NOT select an aggregate assignment_id here because Postgres
    //    `MIN()` is not defined for the `uuid` type. When the Completed
    //    branch fires below, we run a small LIMIT-1 follow-up to fetch any
    //    member of the winning group.
    let groups = sqlx::query_as::<_, HashGroup>(
        r#"
        SELECT result_hash,
               COUNT(*)::bigint AS count
          FROM assignments
         WHERE task_id = $1
           AND status = 'submitted'
           AND result_hash IS NOT NULL
         GROUP BY result_hash
         ORDER BY COUNT(*) DESC, result_hash ASC
        "#,
    )
    .bind(task_id)
    .fetch_all(&mut **tx)
    .await?;

    let submission_count: i64 = groups.iter().map(|g| g.count).sum();

    // 3. Apply the decision table.
    //
    // Defensive: if we're called with 0 submissions there's nothing to
    // decide; leave the task untouched and report Escalated (the caller's
    // recompute then treats it as "still awaiting"). In practice the
    // caller only invokes us with count >= redundancy_target, so this
    // branch is a safety net.
    if submission_count == 0 {
        return Ok(ConsensusOutcome::Escalated);
    }

    // Largest group is first thanks to `ORDER BY COUNT(*) DESC`.
    let majority = &groups[0];
    let distinct_hashes = groups.len();

    // Strict majority: more than half the submissions share a hash →
    // Completed. Covers the `redundancy_target = 1` collapse (1/1), the
    // simple `2/1`, the `3/≤2`, and the defensive `>3` rows of the table.
    // Uses `2 * count > total` to avoid integer-division rounding.
    if 2 * majority.count > submission_count {
        // Fetch any assignment_id from the winning hash group.
        // Ordered by assigned_at ASC to make winner selection
        // deterministic for diagnostics / replay.
        let winning_assignment_id: Uuid = sqlx::query_scalar(
            r#"
            SELECT assignment_id
              FROM assignments
             WHERE task_id = $1
               AND status = 'submitted'
               AND result_hash = $2
             ORDER BY assigned_at ASC
             LIMIT 1
            "#,
        )
        .bind(task_id)
        .bind(majority.result_hash.as_ref())
        .fetch_one(&mut **tx)
        .await?;
        sqlx::query(
            r#"
            UPDATE tasks
               SET status = 'completed',
                   winning_assignment_id = $2
             WHERE task_id = $1
            "#,
        )
        .bind(task_id)
        .bind(winning_assignment_id)
        .execute(&mut **tx)
        .await?;
        return Ok(ConsensusOutcome::Completed);
    }

    // No hash has ≥2 matches, therefore every submission has a unique
    // hash. Branch on submission count:
    //   * submissions = 2, distinct = 2 → Escalated (bump target to 3).
    //   * submissions ≥ 3, distinct = submissions → Failed.
    //   * submissions = 1 should not happen (caller guards on count ≥
    //     redundancy_target ≥ 1) but falls through to Escalated as the
    //     least-destructive choice.
    if submission_count >= 3 && distinct_hashes as i64 == submission_count {
        sqlx::query(r#"UPDATE tasks SET status = 'failed' WHERE task_id = $1"#)
            .bind(task_id)
            .execute(&mut **tx)
            .await?;
        return Ok(ConsensusOutcome::Failed);
    }

    // Exactly two disagreeing submissions → bump redundancy_target so
    // pick_next_task dispatches the task to a third worker. Status stays
    // `awaiting_consensus`; winning_assignment_id stays NULL.
    sqlx::query(
        r#"UPDATE tasks SET redundancy_target = redundancy_target + 1 WHERE task_id = $1"#,
    )
    .bind(task_id)
    .execute(&mut **tx)
    .await?;
    Ok(ConsensusOutcome::Escalated)
}

#[cfg(test)]
mod tests {
    //! Consensus policy tests. Each case seeds a task + Submitted
    //! assignments directly, runs `try_resolve_consensus` inside a real
    //! transaction, commits, and re-reads `tasks` to assert the outcome.
    //!
    //! Tests skip cleanly when `DATABASE_URL` is unset so they don't
    //! block local runs without the Postgres container. See
    //! `modules/task-distribution/src/db.rs::pool_or_skip` for the
    //! matching pattern on that side.

    use super::*;
    use chrono::{Duration, Utc};
    use sqlx::postgres::PgPoolOptions;
    use sqlx::PgPool;

    use crate::test_support::{reset_db, DB_LOCK};

    async fn pool_or_skip()
        -> Option<(PgPool, tokio::sync::MutexGuard<'static, ()>)> {
        let url = std::env::var("DATABASE_URL").ok()?;
        let pool = PgPoolOptions::new()
            .max_connections(4)
            .connect(&url)
            .await
            .expect("DATABASE_URL set but connection failed");
        let guard = DB_LOCK.lock().await;
        reset_db(&pool).await;
        Some((pool, guard))
    }

    async fn insert_job_row(pool: &PgPool, job_id: Uuid, status: &str) {
        sqlx::query(
            r#"
            INSERT INTO jobs (job_id, csv_filename, script_filename,
                              csv_size_bytes, script_size_bytes, status, created_at)
            VALUES ($1, 'data.csv', 'run.py', 10, 5, $2, now())
            "#,
        )
        .bind(job_id)
        .bind(status)
        .execute(pool)
        .await
        .unwrap();
    }

    async fn insert_task_row_rt(
        pool: &PgPool,
        task_id: Uuid,
        job_id: Uuid,
        status: &str,
        redundancy_target: i16,
    ) {
        sqlx::query(
            r#"
            INSERT INTO tasks
                (task_id, job_id, chunk_index, input_rows, status, attempts,
                 redundancy_target, created_at)
            VALUES
                ($1, $2, 0, ARRAY['x'], $3, 0, $4, now())
            "#,
        )
        .bind(task_id)
        .bind(job_id)
        .bind(status)
        .bind(redundancy_target)
        .execute(pool)
        .await
        .unwrap();
    }

    async fn insert_submitted(
        pool: &PgPool,
        assignment_id: Uuid,
        task_id: Uuid,
        result_hash: Option<&str>,
    ) {
        let now = Utc::now();
        sqlx::query(
            r#"
            INSERT INTO assignments
                (assignment_id, task_id, worker_id, assigned_at, deadline_at,
                 status, stdout, stderr, duration_ms, result_hash)
            VALUES
                ($1, $2, $3, $4, $5, 'submitted', 'out', '', 1.0, $6)
            "#,
        )
        .bind(assignment_id)
        .bind(task_id)
        .bind(Uuid::new_v4())
        .bind(now - Duration::seconds(30))
        .bind(now + Duration::seconds(30))
        .bind(result_hash)
        .execute(pool)
        .await
        .unwrap();
    }

    async fn get_task(pool: &PgPool, task_id: Uuid) -> (String, Option<Uuid>, i16) {
        sqlx::query_as::<_, (String, Option<Uuid>, i16)>(
            r#"SELECT status, winning_assignment_id, redundancy_target
                 FROM tasks WHERE task_id = $1"#,
        )
        .bind(task_id)
        .fetch_one(pool)
        .await
        .unwrap()
    }

    /// Run `try_resolve_consensus` inside a fresh tx and commit.
    async fn run_consensus(pool: &PgPool, task_id: Uuid) -> ConsensusOutcome {
        let mut tx = pool.begin().await.unwrap();
        let outcome = try_resolve_consensus(&mut tx, task_id).await.unwrap();
        tx.commit().await.unwrap();
        outcome
    }

    #[tokio::test]
    async fn test_consensus_returns_completed_when_all_match() {
        // 2 submissions, 1 distinct hash → Completed, winning_assignment_id
        // set to one of the matching assignments, target unchanged.
        let Some((pool, _guard)) = pool_or_skip().await else {
            return;
        };

        let job_id = Uuid::new_v4();
        insert_job_row(&pool, job_id, "processing").await;
        let task_id = Uuid::new_v4();
        insert_task_row_rt(&pool, task_id, job_id, "awaiting_consensus", 2).await;

        let a1 = Uuid::new_v4();
        let a2 = Uuid::new_v4();
        insert_submitted(&pool, a1, task_id, Some("aaa")).await;
        insert_submitted(&pool, a2, task_id, Some("aaa")).await;

        let outcome = run_consensus(&pool, task_id).await;
        assert_eq!(outcome, ConsensusOutcome::Completed);

        let (status, winner, target) = get_task(&pool, task_id).await;
        assert_eq!(status, "completed");
        let winner = winner.expect("winning_assignment_id must be set");
        assert!(winner == a1 || winner == a2);
        assert_eq!(target, 2);
    }

    #[tokio::test]
    async fn test_consensus_escalates_on_2_disagree() {
        // 2 submissions, 2 distinct hashes → Escalated, status stays
        // awaiting_consensus, redundancy_target bumped 2 → 3.
        let Some((pool, _guard)) = pool_or_skip().await else {
            return;
        };

        let job_id = Uuid::new_v4();
        insert_job_row(&pool, job_id, "processing").await;
        let task_id = Uuid::new_v4();
        insert_task_row_rt(&pool, task_id, job_id, "awaiting_consensus", 2).await;

        insert_submitted(&pool, Uuid::new_v4(), task_id, Some("aaa")).await;
        insert_submitted(&pool, Uuid::new_v4(), task_id, Some("bbb")).await;

        let outcome = run_consensus(&pool, task_id).await;
        assert_eq!(outcome, ConsensusOutcome::Escalated);

        let (status, winner, target) = get_task(&pool, task_id).await;
        assert_eq!(status, "awaiting_consensus");
        assert!(winner.is_none(), "winning_assignment_id must stay NULL");
        assert_eq!(target, 3, "redundancy_target must be bumped to 3");
    }

    #[tokio::test]
    async fn test_consensus_completes_on_3_with_majority() {
        // 3 submissions: hashes (A, A, B). Majority hash A has count 2 →
        // Completed, winner chosen from the A group.
        let Some((pool, _guard)) = pool_or_skip().await else {
            return;
        };

        let job_id = Uuid::new_v4();
        insert_job_row(&pool, job_id, "processing").await;
        let task_id = Uuid::new_v4();
        insert_task_row_rt(&pool, task_id, job_id, "awaiting_consensus", 3).await;

        let a1 = Uuid::new_v4();
        let a2 = Uuid::new_v4();
        let b = Uuid::new_v4();
        insert_submitted(&pool, a1, task_id, Some("AAA")).await;
        insert_submitted(&pool, a2, task_id, Some("AAA")).await;
        insert_submitted(&pool, b, task_id, Some("BBB")).await;

        let outcome = run_consensus(&pool, task_id).await;
        assert_eq!(outcome, ConsensusOutcome::Completed);

        let (status, winner, target) = get_task(&pool, task_id).await;
        assert_eq!(status, "completed");
        let winner = winner.expect("winning_assignment_id must be set");
        assert!(
            winner == a1 || winner == a2,
            "winner must come from the A group (a1={a1}, a2={a2}), got {winner}"
        );
        assert_eq!(target, 3, "target unchanged on Completed");
    }

    #[tokio::test]
    async fn test_consensus_fails_when_all_3_disagree() {
        // 3 submissions, 3 distinct hashes, no majority → Failed.
        let Some((pool, _guard)) = pool_or_skip().await else {
            return;
        };

        let job_id = Uuid::new_v4();
        insert_job_row(&pool, job_id, "processing").await;
        let task_id = Uuid::new_v4();
        insert_task_row_rt(&pool, task_id, job_id, "awaiting_consensus", 3).await;

        insert_submitted(&pool, Uuid::new_v4(), task_id, Some("AAA")).await;
        insert_submitted(&pool, Uuid::new_v4(), task_id, Some("BBB")).await;
        insert_submitted(&pool, Uuid::new_v4(), task_id, Some("CCC")).await;

        let outcome = run_consensus(&pool, task_id).await;
        assert_eq!(outcome, ConsensusOutcome::Failed);

        let (status, winner, target) = get_task(&pool, task_id).await;
        assert_eq!(status, "failed");
        assert!(winner.is_none());
        assert_eq!(target, 3);
    }

    #[tokio::test]
    async fn test_consensus_ignores_null_hashes() {
        // Defense-in-depth: even if two Submitted assignments carry NULL
        // hashes (shouldn't happen in practice — `submit_task` always sets
        // the hash), NULL rows must NOT form a majority. The SQL filter
        // drops them, so the resulting group count is 0 → Escalated.
        let Some((pool, _guard)) = pool_or_skip().await else {
            return;
        };

        let job_id = Uuid::new_v4();
        insert_job_row(&pool, job_id, "processing").await;
        let task_id = Uuid::new_v4();
        insert_task_row_rt(&pool, task_id, job_id, "awaiting_consensus", 2).await;

        insert_submitted(&pool, Uuid::new_v4(), task_id, None).await;
        insert_submitted(&pool, Uuid::new_v4(), task_id, None).await;

        let outcome = run_consensus(&pool, task_id).await;
        assert_eq!(
            outcome,
            ConsensusOutcome::Escalated,
            "NULL hashes must not form a majority"
        );

        let (status, winner, _target) = get_task(&pool, task_id).await;
        assert_eq!(status, "awaiting_consensus");
        assert!(winner.is_none());
    }
}
