use chrono::{DateTime, Duration, Utc};
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use foinc_integrations::TaskStats;

/// Assignment deadline in seconds from dispatch time.
pub(crate) const DEADLINE_SECS: i64 = 60;

/// Maximum number of dispatch attempts per task. Once `attempts` reaches
/// this value an expired assignment moves the task to `Failed` instead of
/// back to `Pending`.
pub(crate) const MAX_ATTEMPTS: i32 = 5;

/// Outcome of [`start_processing`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartProcessingOutcome {
    /// Job did not exist at all.
    NotFound,
    /// Job exists but is not in `uploaded` status.
    Conflict,
    /// CAS succeeded and the job is now `processing`.
    Started,
}

/// Attempt to move a job from `uploaded` into `processing`. Returns
/// [`StartProcessingOutcome::Started`] only for the caller that actually
/// won the atomic `UPDATE ... WHERE status='uploaded' RETURNING` race.
pub async fn start_processing(
    pool: &PgPool,
    job_id: Uuid,
) -> Result<StartProcessingOutcome, sqlx::Error> {
    // First try the atomic CAS.
    let updated = sqlx::query_scalar::<_, Uuid>(
        r#"
        UPDATE jobs
           SET status = 'processing'
         WHERE job_id = $1
           AND status = 'uploaded'
         RETURNING job_id
        "#,
    )
    .bind(job_id)
    .fetch_optional(pool)
    .await?;

    if updated.is_some() {
        return Ok(StartProcessingOutcome::Started);
    }

    // Distinguish "no such job" from "wrong status" for the HTTP layer.
    let exists = sqlx::query_scalar::<_, Uuid>(
        r#"SELECT job_id FROM jobs WHERE job_id = $1"#,
    )
    .bind(job_id)
    .fetch_optional(pool)
    .await?;

    Ok(match exists {
        Some(_) => StartProcessingOutcome::Conflict,
        None => StartProcessingOutcome::NotFound,
    })
}

/// Bulk-insert tasks in `Pending` state for the given job. Caller has
/// already moved the job into `processing` via [`start_processing`].
pub async fn insert_pending_tasks(
    pool: &PgPool,
    job_id: Uuid,
    rows: &[String],
) -> Result<u32, sqlx::Error> {
    let mut tx = pool.begin().await?;

    for (idx, line) in rows.iter().enumerate() {
        let task_id = Uuid::new_v4();
        // `input_rows` is a Postgres TEXT[]; pass as a slice.
        let input_rows = vec![line.clone()];
        sqlx::query(
            r#"
            INSERT INTO tasks
                (task_id, job_id, chunk_index, input_rows, status, attempts, created_at)
            VALUES
                ($1, $2, $3, $4, 'pending', 0, now())
            "#,
        )
        .bind(task_id)
        .bind(job_id)
        .bind(idx as i32)
        .bind(&input_rows)
        .execute(&mut *tx)
        .await?;
    }

    tx.commit().await?;
    Ok(rows.len() as u32)
}

/// Result of [`pick_next_task`].
#[derive(Debug, Clone)]
pub struct PickedTask {
    pub task_id: Uuid,
    pub job_id: Uuid,
    pub input_rows: Vec<String>,
    pub deadline_at: DateTime<Utc>,
}

/// Atomically pick the next task eligible for dispatch and create an
/// in-flight assignment for `worker_id`. Returns `None` when no task is
/// currently eligible.
///
/// Concurrency: uses `SELECT ... FOR UPDATE SKIP LOCKED` on the `tasks`
/// row so concurrent pickers never contend on the same task. The whole
/// pick-and-assign runs inside a single transaction that also mutates the
/// old assignment (on reclamation). The handler loops for another
/// candidate if the current one exhausted its retry budget.
pub async fn pick_next_task(
    pool: &PgPool,
    worker_id: Uuid,
) -> Result<Option<PickedTask>, sqlx::Error> {
    loop {
        let mut tx: Transaction<'_, Postgres> = pool.begin().await?;

        // Candidate = Pending task, OR Assigned task whose current InFlight
        // assignment's deadline_at < now(). Order to prefer older work.
        let row = sqlx::query_as::<_, CandidateRow>(
            r#"
            SELECT t.task_id, t.job_id, t.status, t.attempts, t.input_rows,
                   a.assignment_id AS expired_assignment_id
              FROM tasks t
         LEFT JOIN LATERAL (
                SELECT assignment_id, deadline_at
                  FROM assignments
                 WHERE task_id = t.task_id
                   AND status = 'in_flight'
                 ORDER BY assigned_at DESC
                 LIMIT 1
              ) a ON TRUE
             WHERE t.status = 'pending'
                OR (t.status = 'assigned' AND a.deadline_at < now())
             ORDER BY t.created_at ASC
             FOR UPDATE OF t SKIP LOCKED
             LIMIT 1
            "#,
        )
        .fetch_optional(&mut *tx)
        .await?;

        let Some(cand) = row else {
            tx.commit().await?;
            return Ok(None);
        };

        // Reclamation path: the candidate is an expired Assigned task.
        if cand.status == "assigned" {
            // Evaluate the attempts cap BEFORE any mutation.
            if cand.attempts >= MAX_ATTEMPTS {
                // Mark the old assignment TimedOut and fail the task. Don't
                // create a new assignment; loop to find another candidate.
                if let Some(old) = cand.expired_assignment_id {
                    sqlx::query(
                        r#"UPDATE assignments SET status = 'timed_out' WHERE assignment_id = $1"#,
                    )
                    .bind(old)
                    .execute(&mut *tx)
                    .await?;
                }
                sqlx::query(r#"UPDATE tasks SET status = 'failed' WHERE task_id = $1"#)
                    .bind(cand.task_id)
                    .execute(&mut *tx)
                    .await?;
                tx.commit().await?;
                continue;
            }

            // Retry path: timeout -> increment attempts, mark old
            // assignment TimedOut. The task status becomes Assigned again
            // once we insert the new assignment below.
            if let Some(old) = cand.expired_assignment_id {
                sqlx::query(
                    r#"UPDATE assignments SET status = 'timed_out' WHERE assignment_id = $1"#,
                )
                .bind(old)
                .execute(&mut *tx)
                .await?;
            }
            sqlx::query(
                r#"UPDATE tasks SET attempts = attempts + 1 WHERE task_id = $1"#,
            )
            .bind(cand.task_id)
            .execute(&mut *tx)
            .await?;
        }

        // Insert new InFlight assignment and mark task Assigned.
        let assignment_id = Uuid::new_v4();
        let now = Utc::now();
        let deadline_at = now + Duration::seconds(DEADLINE_SECS);

        sqlx::query(
            r#"
            INSERT INTO assignments
                (assignment_id, task_id, worker_id, assigned_at, deadline_at, status)
            VALUES
                ($1, $2, $3, $4, $5, 'in_flight')
            "#,
        )
        .bind(assignment_id)
        .bind(cand.task_id)
        .bind(worker_id)
        .bind(now)
        .bind(deadline_at)
        .execute(&mut *tx)
        .await?;

        sqlx::query(r#"UPDATE tasks SET status = 'assigned' WHERE task_id = $1"#)
            .bind(cand.task_id)
            .execute(&mut *tx)
            .await?;

        tx.commit().await?;

        return Ok(Some(PickedTask {
            task_id: cand.task_id,
            job_id: cand.job_id,
            input_rows: cand.input_rows,
            deadline_at,
        }));
    }
}

#[derive(sqlx::FromRow)]
struct CandidateRow {
    task_id: Uuid,
    job_id: Uuid,
    status: String,
    attempts: i32,
    input_rows: Vec<String>,
    expired_assignment_id: Option<Uuid>,
}

/// Outcome of [`submit_task`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubmitOutcome {
    /// Task does not exist, or has no assignment at all.
    NotFound,
    /// The current assignment does not match the submitter (wrong worker,
    /// wrong status, or deadline passed).
    Conflict,
    /// Submission accepted; assignment and task marked terminal.
    Submitted {
        /// New job status after siblings were re-evaluated, or `None` when
        /// the job remains in `processing`.
        job_terminal: Option<JobTerminal>,
    },
}

/// Either `Completed` or `Failed` — the two terminal states the submit
/// path can flip a job into.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobTerminal {
    Completed,
    Failed,
}

/// Persist a submission for the task, then recompute parent job terminality.
pub async fn submit_task(
    pool: &PgPool,
    task_id: Uuid,
    worker_id: Uuid,
    stdout: &str,
    stderr: &str,
    duration_ms: f64,
) -> Result<SubmitOutcome, sqlx::Error> {
    let mut tx: Transaction<'_, Postgres> = pool.begin().await?;

    // First make sure the task exists — otherwise 404.
    let task_row = sqlx::query_scalar::<_, Uuid>(
        r#"SELECT task_id FROM tasks WHERE task_id = $1 FOR UPDATE"#,
    )
    .bind(task_id)
    .fetch_optional(&mut *tx)
    .await?;

    if task_row.is_none() {
        tx.rollback().await?;
        return Ok(SubmitOutcome::NotFound);
    }

    // Lock the most recent assignment row for this task.
    let current = sqlx::query_as::<_, CurrentAssignmentRow>(
        r#"
        SELECT assignment_id, worker_id, deadline_at, status
          FROM assignments
         WHERE task_id = $1
         ORDER BY assigned_at DESC
         LIMIT 1
         FOR UPDATE
        "#,
    )
    .bind(task_id)
    .fetch_optional(&mut *tx)
    .await?;

    let Some(curr) = current else {
        tx.rollback().await?;
        return Ok(SubmitOutcome::NotFound);
    };

    // Validate under the row lock.
    if curr.status != "in_flight" || curr.worker_id != worker_id || curr.deadline_at < Utc::now() {
        tx.rollback().await?;
        return Ok(SubmitOutcome::Conflict);
    }

    // Accept the submission.
    sqlx::query(
        r#"
        UPDATE assignments
           SET status = 'submitted',
               stdout = $2,
               stderr = $3,
               duration_ms = $4
         WHERE assignment_id = $1
        "#,
    )
    .bind(curr.assignment_id)
    .bind(stdout)
    .bind(stderr)
    .bind(duration_ms)
    .execute(&mut *tx)
    .await?;

    sqlx::query(r#"UPDATE tasks SET status = 'completed' WHERE task_id = $1"#)
        .bind(task_id)
        .execute(&mut *tx)
        .await?;

    // Find the job and recompute terminality based on siblings.
    let job_id: Uuid = sqlx::query_scalar(r#"SELECT job_id FROM tasks WHERE task_id = $1"#)
        .bind(task_id)
        .fetch_one(&mut *tx)
        .await?;

    let sibling_counts = sqlx::query_as::<_, SiblingCounts>(
        r#"
        SELECT
            COUNT(*) FILTER (WHERE status NOT IN ('completed', 'failed')) AS non_terminal,
            COUNT(*) FILTER (WHERE status = 'completed') AS completed
          FROM tasks
         WHERE job_id = $1
        "#,
    )
    .bind(job_id)
    .fetch_one(&mut *tx)
    .await?;

    let mut job_terminal: Option<JobTerminal> = None;
    if sibling_counts.non_terminal == 0 {
        if sibling_counts.completed > 0 {
            sqlx::query(
                r#"UPDATE jobs SET status = 'completed' WHERE job_id = $1 AND status = 'processing'"#,
            )
            .bind(job_id)
            .execute(&mut *tx)
            .await?;
            job_terminal = Some(JobTerminal::Completed);
        } else {
            sqlx::query(
                r#"UPDATE jobs SET status = 'failed' WHERE job_id = $1 AND status = 'processing'"#,
            )
            .bind(job_id)
            .execute(&mut *tx)
            .await?;
            job_terminal = Some(JobTerminal::Failed);
        }
    }

    tx.commit().await?;

    Ok(SubmitOutcome::Submitted { job_terminal })
}

#[derive(sqlx::FromRow)]
struct CurrentAssignmentRow {
    assignment_id: Uuid,
    worker_id: Uuid,
    deadline_at: DateTime<Utc>,
    status: String,
}

#[derive(sqlx::FromRow)]
struct SiblingCounts {
    non_terminal: i64,
    completed: i64,
}

/// Check whether a job row exists.
pub async fn job_exists(pool: &PgPool, job_id: Uuid) -> Result<bool, sqlx::Error> {
    let row = sqlx::query_scalar::<_, Uuid>(r#"SELECT job_id FROM jobs WHERE job_id = $1"#)
        .bind(job_id)
        .fetch_optional(pool)
        .await?;
    Ok(row.is_some())
}

/// Compute current task statistics for a (job, worker) pair.
pub async fn get_task_stats(
    pool: &PgPool,
    job_id: Uuid,
    worker_id: Uuid,
) -> Result<TaskStats, sqlx::Error> {
    let pending: i64 = sqlx::query_scalar(
        r#"SELECT COUNT(*) FROM tasks WHERE job_id = $1 AND status = 'pending'"#,
    )
    .bind(job_id)
    .fetch_one(pool)
    .await?;

    let in_flight: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(*)
          FROM tasks t
          JOIN LATERAL (
                SELECT deadline_at, status
                  FROM assignments
                 WHERE task_id = t.task_id
                 ORDER BY assigned_at DESC
                 LIMIT 1
              ) a ON TRUE
         WHERE t.job_id = $1
           AND t.status = 'assigned'
           AND a.status = 'in_flight'
           AND a.deadline_at >= now()
        "#,
    )
    .bind(job_id)
    .fetch_one(pool)
    .await?;

    let completed_total: i64 = sqlx::query_scalar(
        r#"SELECT COUNT(*) FROM tasks WHERE job_id = $1 AND status = 'completed'"#,
    )
    .bind(job_id)
    .fetch_one(pool)
    .await?;

    let completed_by_me: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(*)
          FROM assignments a
          JOIN tasks t ON t.task_id = a.task_id
         WHERE t.job_id = $1
           AND a.worker_id = $2
           AND a.status = 'submitted'
        "#,
    )
    .bind(job_id)
    .bind(worker_id)
    .fetch_one(pool)
    .await?;

    Ok(TaskStats {
        pending,
        in_flight,
        completed_total,
        completed_by_me,
    })
}

/// Read a job's script file from disk.
///
/// The upload module writes `data/{job_id}/{script_filename}`. This helper
/// locates the first `.py` file in the job directory and returns its
/// contents. Returns an `io::Error` if the directory or file is missing.
pub async fn read_job_script(job_id: Uuid) -> Result<String, std::io::Error> {
    let data_dir = std::env::var("DATA_DIR").unwrap_or_else(|_| "data".to_string());
    let dir = std::path::PathBuf::from(data_dir).join(job_id.to_string());
    let mut read = tokio::fs::read_dir(&dir).await?;
    while let Some(entry) = read.next_entry().await? {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) == Some("py") {
            return tokio::fs::read_to_string(&path).await;
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::NotFound,
        "no .py script found in job directory",
    ))
}

/// Locate the CSV file for a job on disk.
pub async fn find_job_csv(job_id: Uuid) -> Result<std::path::PathBuf, std::io::Error> {
    let data_dir = std::env::var("DATA_DIR").unwrap_or_else(|_| "data".to_string());
    let dir = std::path::PathBuf::from(data_dir).join(job_id.to_string());
    let mut read = tokio::fs::read_dir(&dir).await?;
    while let Some(entry) = read.next_entry().await? {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) == Some("csv") {
            return Ok(path);
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::NotFound,
        "no .csv file found in job directory",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, Utc};
    use sqlx::postgres::PgPoolOptions;

    use crate::test_support::{reset_db, DB_LOCK};

    /// Connect to the test Postgres instance configured via `DATABASE_URL`.
    ///
    /// Returns `None` when the env var is unset so local runs without the DB
    /// stack simply skip these integration-flavoured tests instead of
    /// failing. CI and any developer running docker-compose will have the
    /// var set and exercise the real DB. Mirrors the pattern from
    /// `modules/upload/src/db.rs`. On success, the test holds the
    /// crate-wide `DB_LOCK` and has just TRUNCATEd the tables — tests in
    /// this crate scan the global `tasks` table, so parallel execution
    /// would otherwise see each other's fixtures.
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

    /// Insert a job row in a chosen status. The `jobs` table has no CHECK
    /// constraint in 001/003 so we can insert any status string we want.
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

    /// Insert a task row directly. Bypasses `insert_pending_tasks` so we can
    /// craft test fixtures in any status.
    async fn insert_task_row(
        pool: &PgPool,
        task_id: Uuid,
        job_id: Uuid,
        status: &str,
        attempts: i32,
        input_rows: &[&str],
    ) {
        let rows: Vec<String> = input_rows.iter().map(|s| s.to_string()).collect();
        sqlx::query(
            r#"
            INSERT INTO tasks
                (task_id, job_id, chunk_index, input_rows, status, attempts, created_at)
            VALUES
                ($1, $2, 0, $3, $4, $5, now())
            "#,
        )
        .bind(task_id)
        .bind(job_id)
        .bind(&rows)
        .bind(status)
        .bind(attempts)
        .execute(pool)
        .await
        .unwrap();
    }

    /// Insert an assignment row directly with an arbitrary `deadline_at` so
    /// we can construct both live and expired scenarios without real sleeps.
    async fn insert_assignment_row(
        pool: &PgPool,
        assignment_id: Uuid,
        task_id: Uuid,
        worker_id: Uuid,
        assigned_at: chrono::DateTime<Utc>,
        deadline_at: chrono::DateTime<Utc>,
        status: &str,
    ) {
        sqlx::query(
            r#"
            INSERT INTO assignments
                (assignment_id, task_id, worker_id, assigned_at, deadline_at, status)
            VALUES
                ($1, $2, $3, $4, $5, $6)
            "#,
        )
        .bind(assignment_id)
        .bind(task_id)
        .bind(worker_id)
        .bind(assigned_at)
        .bind(deadline_at)
        .bind(status)
        .execute(pool)
        .await
        .unwrap();
    }

    /// Fetch `(status, attempts)` for a task; used for assertions.
    async fn get_task_status_and_attempts(pool: &PgPool, task_id: Uuid) -> (String, i32) {
        sqlx::query_as::<_, (String, i32)>(
            r#"SELECT status, attempts FROM tasks WHERE task_id = $1"#,
        )
        .bind(task_id)
        .fetch_one(pool)
        .await
        .unwrap()
    }

    /// Fetch an assignment's status.
    async fn get_assignment_status(pool: &PgPool, assignment_id: Uuid) -> String {
        sqlx::query_scalar::<_, String>(
            r#"SELECT status FROM assignments WHERE assignment_id = $1"#,
        )
        .bind(assignment_id)
        .fetch_one(pool)
        .await
        .unwrap()
    }

    /// Fetch a job's status.
    async fn get_job_status(pool: &PgPool, job_id: Uuid) -> String {
        sqlx::query_scalar::<_, String>(r#"SELECT status FROM jobs WHERE job_id = $1"#)
            .bind(job_id)
            .fetch_one(pool)
            .await
            .unwrap()
    }

    // -------------------------------------------------------------------
    // start_processing
    // -------------------------------------------------------------------

    #[tokio::test]
    async fn test_start_processing_flips_status_atomically() {
        let Some((pool, _guard)) = pool_or_skip().await else {
            return;
        };

        let job_id = Uuid::new_v4();
        insert_job_row(&pool, job_id, "uploaded").await;

        let outcome = start_processing(&pool, job_id).await.unwrap();
        assert_eq!(outcome, StartProcessingOutcome::Started);

        assert_eq!(get_job_status(&pool, job_id).await, "processing");
    }

    #[tokio::test]
    async fn test_start_processing_returns_none_on_already_processing() {
        let Some((pool, _guard)) = pool_or_skip().await else {
            return;
        };

        let job_id = Uuid::new_v4();
        insert_job_row(&pool, job_id, "processing").await;

        let outcome = start_processing(&pool, job_id).await.unwrap();
        assert_eq!(outcome, StartProcessingOutcome::Conflict);

        // Status must remain untouched.
        assert_eq!(get_job_status(&pool, job_id).await, "processing");
    }

    #[tokio::test]
    async fn test_start_processing_returns_not_found_for_missing_job() {
        let Some((pool, _guard)) = pool_or_skip().await else {
            return;
        };

        let outcome = start_processing(&pool, Uuid::new_v4()).await.unwrap();
        assert_eq!(outcome, StartProcessingOutcome::NotFound);
    }

    // -------------------------------------------------------------------
    // insert_pending_tasks
    // -------------------------------------------------------------------

    #[tokio::test]
    async fn test_insert_pending_tasks_creates_one_task_per_row() {
        let Some((pool, _guard)) = pool_or_skip().await else {
            return;
        };

        let job_id = Uuid::new_v4();
        insert_job_row(&pool, job_id, "processing").await;

        let rows = vec![
            "1,2".to_string(),
            "3,4".to_string(),
            "5,6".to_string(),
        ];
        let n = insert_pending_tasks(&pool, job_id, &rows).await.unwrap();
        assert_eq!(n, 3);

        let count: i64 = sqlx::query_scalar(
            r#"SELECT COUNT(*) FROM tasks WHERE job_id = $1 AND status = 'pending'"#,
        )
        .bind(job_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(count, 3);

        // input_rows length-1 invariant: every task has exactly one element.
        let lengths: Vec<i32> = sqlx::query_scalar(
            r#"SELECT array_length(input_rows, 1) FROM tasks WHERE job_id = $1"#,
        )
        .bind(job_id)
        .fetch_all(&pool)
        .await
        .unwrap();
        assert!(lengths.iter().all(|l| *l == 1), "every task must have input_rows length 1");
    }

    // -------------------------------------------------------------------
    // pick_next_task
    // -------------------------------------------------------------------

    #[tokio::test]
    async fn test_pick_next_task_picks_pending_first() {
        let Some((pool, _guard)) = pool_or_skip().await else {
            return;
        };

        let job_id = Uuid::new_v4();
        insert_job_row(&pool, job_id, "processing").await;
        let task_id = Uuid::new_v4();
        insert_task_row(&pool, task_id, job_id, "pending", 0, &["42,7"]).await;

        let worker_id = Uuid::new_v4();
        let picked = pick_next_task(&pool, worker_id).await.unwrap();
        let picked = picked.expect("should pick the pending task");
        assert_eq!(picked.task_id, task_id);
        assert_eq!(picked.input_rows, vec!["42,7".to_string()]);

        // Task flipped to Assigned.
        let (status, attempts) = get_task_status_and_attempts(&pool, task_id).await;
        assert_eq!(status, "assigned");
        // Initial dispatch must NOT increment attempts.
        assert_eq!(attempts, 0);

        // And there's a live InFlight assignment for our worker.
        let live: (Uuid, String) = sqlx::query_as(
            r#"SELECT worker_id, status FROM assignments WHERE task_id = $1 ORDER BY assigned_at DESC LIMIT 1"#,
        )
        .bind(task_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(live.0, worker_id);
        assert_eq!(live.1, "in_flight");
    }

    #[tokio::test]
    async fn test_pick_next_task_reclaims_expired_assignment() {
        let Some((pool, _guard)) = pool_or_skip().await else {
            return;
        };

        let job_id = Uuid::new_v4();
        insert_job_row(&pool, job_id, "processing").await;

        let task_id = Uuid::new_v4();
        insert_task_row(&pool, task_id, job_id, "assigned", 1, &["x,y"]).await;

        let old_assignment = Uuid::new_v4();
        let original_worker = Uuid::new_v4();
        let now = Utc::now();
        insert_assignment_row(
            &pool,
            old_assignment,
            task_id,
            original_worker,
            now - Duration::seconds(120),
            now - Duration::seconds(60), // expired
            "in_flight",
        )
        .await;

        let new_worker = Uuid::new_v4();
        let picked = pick_next_task(&pool, new_worker)
            .await
            .unwrap()
            .expect("should reclaim the expired assignment");
        assert_eq!(picked.task_id, task_id);

        // Old assignment must be TimedOut.
        assert_eq!(get_assignment_status(&pool, old_assignment).await, "timed_out");

        // Task must be Assigned again (new assignment), and attempts bumped.
        let (status, attempts) = get_task_status_and_attempts(&pool, task_id).await;
        assert_eq!(status, "assigned");
        assert_eq!(attempts, 2, "attempts increments on reclamation");

        // A fresh InFlight assignment exists for the new worker.
        let latest: (Uuid, String) = sqlx::query_as(
            r#"SELECT worker_id, status FROM assignments WHERE task_id = $1 ORDER BY assigned_at DESC LIMIT 1"#,
        )
        .bind(task_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(latest.0, new_worker);
        assert_eq!(latest.1, "in_flight");
    }

    #[tokio::test]
    async fn test_pick_next_task_marks_task_failed_after_max_attempts() {
        let Some((pool, _guard)) = pool_or_skip().await else {
            return;
        };

        let job_id = Uuid::new_v4();
        insert_job_row(&pool, job_id, "processing").await;

        let task_id = Uuid::new_v4();
        // attempts already at MAX_ATTEMPTS; next expired sighting must fail it.
        insert_task_row(&pool, task_id, job_id, "assigned", MAX_ATTEMPTS, &["exhausted"]).await;

        let old_assignment = Uuid::new_v4();
        let now = Utc::now();
        insert_assignment_row(
            &pool,
            old_assignment,
            task_id,
            Uuid::new_v4(),
            now - Duration::seconds(120),
            now - Duration::seconds(60),
            "in_flight",
        )
        .await;

        let worker_id = Uuid::new_v4();
        let picked = pick_next_task(&pool, worker_id).await.unwrap();
        assert!(picked.is_none(), "handler loops until no candidate remains");

        // Old assignment TimedOut, task Failed.
        assert_eq!(get_assignment_status(&pool, old_assignment).await, "timed_out");
        let (status, attempts) = get_task_status_and_attempts(&pool, task_id).await;
        assert_eq!(status, "failed");
        // Attempts must NOT have been incremented past MAX — the cap check
        // runs before any mutation.
        assert_eq!(attempts, MAX_ATTEMPTS);
    }

    #[tokio::test]
    async fn test_pick_next_task_returns_none_when_queue_empty() {
        let Some((pool, _guard)) = pool_or_skip().await else {
            return;
        };

        // No matching pending tasks for the random worker — nothing to do.
        let picked = pick_next_task(&pool, Uuid::new_v4()).await.unwrap();
        assert!(picked.is_none());
    }

    #[tokio::test]
    async fn test_pick_next_task_skips_locked() {
        // Smoke test: run two pickers concurrently against a single Pending
        // task; exactly one should receive the dispatch. This is a basic
        // SKIP LOCKED check; if scheduling makes it flaky in CI we can
        // mark it #[ignore].
        let Some((pool, _guard)) = pool_or_skip().await else {
            return;
        };

        let job_id = Uuid::new_v4();
        insert_job_row(&pool, job_id, "processing").await;
        let task_id = Uuid::new_v4();
        insert_task_row(&pool, task_id, job_id, "pending", 0, &["solo"]).await;

        let p1 = pool.clone();
        let p2 = pool.clone();
        let w1 = Uuid::new_v4();
        let w2 = Uuid::new_v4();

        let (r1, r2) = tokio::join!(
            tokio::spawn(async move { pick_next_task(&p1, w1).await.unwrap() }),
            tokio::spawn(async move { pick_next_task(&p2, w2).await.unwrap() }),
        );
        let r1 = r1.unwrap();
        let r2 = r2.unwrap();

        let got: Vec<_> = [r1, r2].into_iter().flatten().collect();
        assert_eq!(
            got.len(),
            1,
            "exactly one of the concurrent pickers should claim the task"
        );
        assert_eq!(got[0].task_id, task_id);
    }

    // -------------------------------------------------------------------
    // submit_task
    // -------------------------------------------------------------------

    #[tokio::test]
    async fn test_submit_task_happy_path() {
        let Some((pool, _guard)) = pool_or_skip().await else {
            return;
        };

        let job_id = Uuid::new_v4();
        insert_job_row(&pool, job_id, "processing").await;

        let task_id = Uuid::new_v4();
        insert_task_row(&pool, task_id, job_id, "assigned", 0, &["a,b"]).await;

        let assignment_id = Uuid::new_v4();
        let worker_id = Uuid::new_v4();
        let now = Utc::now();
        insert_assignment_row(
            &pool,
            assignment_id,
            task_id,
            worker_id,
            now,
            now + Duration::seconds(60),
            "in_flight",
        )
        .await;

        let outcome = submit_task(&pool, task_id, worker_id, "out", "err", 12.5)
            .await
            .unwrap();
        assert_eq!(
            outcome,
            SubmitOutcome::Submitted {
                job_terminal: Some(JobTerminal::Completed)
            }
        );

        assert_eq!(get_assignment_status(&pool, assignment_id).await, "submitted");
        assert_eq!(get_task_status_and_attempts(&pool, task_id).await.0, "completed");
        assert_eq!(get_job_status(&pool, job_id).await, "completed");

        // stdout/stderr/duration persisted.
        let (stdout, stderr, duration): (Option<String>, Option<String>, Option<f64>) =
            sqlx::query_as(
                r#"SELECT stdout, stderr, duration_ms FROM assignments WHERE assignment_id = $1"#,
            )
            .bind(assignment_id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(stdout.as_deref(), Some("out"));
        assert_eq!(stderr.as_deref(), Some("err"));
        assert!((duration.unwrap() - 12.5).abs() < 1e-9);
    }

    #[tokio::test]
    async fn test_submit_task_rejects_worker_id_mismatch() {
        let Some((pool, _guard)) = pool_or_skip().await else {
            return;
        };

        let job_id = Uuid::new_v4();
        insert_job_row(&pool, job_id, "processing").await;
        let task_id = Uuid::new_v4();
        insert_task_row(&pool, task_id, job_id, "assigned", 0, &["a"]).await;
        let assignment_id = Uuid::new_v4();
        let real_worker = Uuid::new_v4();
        let now = Utc::now();
        insert_assignment_row(
            &pool,
            assignment_id,
            task_id,
            real_worker,
            now,
            now + Duration::seconds(60),
            "in_flight",
        )
        .await;

        let outcome = submit_task(&pool, task_id, Uuid::new_v4(), "o", "e", 1.0)
            .await
            .unwrap();
        assert_eq!(outcome, SubmitOutcome::Conflict);

        // State should be untouched.
        assert_eq!(get_assignment_status(&pool, assignment_id).await, "in_flight");
        assert_eq!(get_task_status_and_attempts(&pool, task_id).await.0, "assigned");
        assert_eq!(get_job_status(&pool, job_id).await, "processing");
    }

    #[tokio::test]
    async fn test_submit_task_rejects_expired_assignment() {
        let Some((pool, _guard)) = pool_or_skip().await else {
            return;
        };

        let job_id = Uuid::new_v4();
        insert_job_row(&pool, job_id, "processing").await;
        let task_id = Uuid::new_v4();
        insert_task_row(&pool, task_id, job_id, "assigned", 0, &["a"]).await;
        let assignment_id = Uuid::new_v4();
        let worker_id = Uuid::new_v4();
        let now = Utc::now();
        // deadline in the past
        insert_assignment_row(
            &pool,
            assignment_id,
            task_id,
            worker_id,
            now - Duration::seconds(120),
            now - Duration::seconds(1),
            "in_flight",
        )
        .await;

        let outcome = submit_task(&pool, task_id, worker_id, "o", "e", 1.0)
            .await
            .unwrap();
        assert_eq!(outcome, SubmitOutcome::Conflict);

        assert_eq!(get_assignment_status(&pool, assignment_id).await, "in_flight");
        assert_eq!(get_task_status_and_attempts(&pool, task_id).await.0, "assigned");
    }

    #[tokio::test]
    async fn test_submit_task_returns_not_found_for_missing_task() {
        let Some((pool, _guard)) = pool_or_skip().await else {
            return;
        };

        let outcome = submit_task(&pool, Uuid::new_v4(), Uuid::new_v4(), "", "", 0.0)
            .await
            .unwrap();
        assert_eq!(outcome, SubmitOutcome::NotFound);
    }

    #[tokio::test]
    async fn test_submit_task_flips_job_to_completed_when_last_sibling_submits() {
        // Mirror of the "flip to failed when all failed" scenario. Under the
        // current submit_task code, the submitter is ALWAYS marked
        // `completed` before sibling counts are recomputed — so when a
        // submit closes out a job, at least one task is Completed and the
        // job moves to `completed`. The `failed` branch
        // (SubmitOutcome::Submitted { job_terminal: Some(Failed) }) is
        // defensively implemented but unreachable from the submit path
        // alone; see `test_pick_next_task_does_not_touch_job_status` for
        // the complementary invariant.
        let Some((pool, _guard)) = pool_or_skip().await else {
            return;
        };

        let job_id = Uuid::new_v4();
        insert_job_row(&pool, job_id, "processing").await;

        // One sibling that already Failed via the reclamation path.
        let failed_sibling = Uuid::new_v4();
        insert_task_row(&pool, failed_sibling, job_id, "failed", MAX_ATTEMPTS, &["x"]).await;

        // One more task which we will successfully submit. With this submit
        // the job moves out of `processing`.
        let submitting_task = Uuid::new_v4();
        insert_task_row(&pool, submitting_task, job_id, "assigned", 0, &["z"]).await;
        let submitting_assignment = Uuid::new_v4();
        let submitting_worker = Uuid::new_v4();
        let now = Utc::now();
        insert_assignment_row(
            &pool,
            submitting_assignment,
            submitting_task,
            submitting_worker,
            now,
            now + Duration::seconds(60),
            "in_flight",
        )
        .await;

        let outcome = submit_task(&pool, submitting_task, submitting_worker, "ok", "", 2.0)
            .await
            .unwrap();
        assert_eq!(
            outcome,
            SubmitOutcome::Submitted {
                job_terminal: Some(JobTerminal::Completed)
            }
        );
        assert_eq!(get_job_status(&pool, job_id).await, "completed");
    }

    #[tokio::test]
    async fn test_pick_next_task_does_not_touch_job_status() {
        // Spec: submit_task is the only path that flips Job out of
        // `processing`. Even when pick_next_task exhausts the last
        // non-terminal task and marks it `failed`, the job must stay in
        // `processing` — only a subsequent submit would flip it (and by
        // construction, the failed branch of that is unreachable).
        let Some((pool, _guard)) = pool_or_skip().await else {
            return;
        };

        let job_id = Uuid::new_v4();
        insert_job_row(&pool, job_id, "processing").await;

        let dying_task = Uuid::new_v4();
        insert_task_row(&pool, dying_task, job_id, "assigned", MAX_ATTEMPTS, &["y"]).await;
        let now = Utc::now();
        insert_assignment_row(
            &pool,
            Uuid::new_v4(),
            dying_task,
            Uuid::new_v4(),
            now - Duration::seconds(120),
            now - Duration::seconds(60),
            "in_flight",
        )
        .await;

        let _ = pick_next_task(&pool, Uuid::new_v4()).await.unwrap();
        assert_eq!(get_task_status_and_attempts(&pool, dying_task).await.0, "failed");
        // Critical invariant.
        assert_eq!(get_job_status(&pool, job_id).await, "processing");
    }

    #[tokio::test]
    async fn test_submit_task_keeps_job_processing_when_siblings_outstanding() {
        let Some((pool, _guard)) = pool_or_skip().await else {
            return;
        };

        let job_id = Uuid::new_v4();
        insert_job_row(&pool, job_id, "processing").await;

        // Sibling still Pending — job must stay in processing after submit.
        let sibling = Uuid::new_v4();
        insert_task_row(&pool, sibling, job_id, "pending", 0, &["a"]).await;

        let task_id = Uuid::new_v4();
        insert_task_row(&pool, task_id, job_id, "assigned", 0, &["b"]).await;
        let assignment_id = Uuid::new_v4();
        let worker_id = Uuid::new_v4();
        let now = Utc::now();
        insert_assignment_row(
            &pool,
            assignment_id,
            task_id,
            worker_id,
            now,
            now + Duration::seconds(60),
            "in_flight",
        )
        .await;

        let outcome = submit_task(&pool, task_id, worker_id, "", "", 0.0)
            .await
            .unwrap();
        assert_eq!(outcome, SubmitOutcome::Submitted { job_terminal: None });
        assert_eq!(get_job_status(&pool, job_id).await, "processing");
    }

    // -------------------------------------------------------------------
    // get_task_stats / job_exists
    // -------------------------------------------------------------------

    #[tokio::test]
    async fn test_get_task_stats_returns_correct_counts() {
        let Some((pool, _guard)) = pool_or_skip().await else {
            return;
        };

        let job_id = Uuid::new_v4();
        insert_job_row(&pool, job_id, "processing").await;
        let me = Uuid::new_v4();
        let other = Uuid::new_v4();
        let now = Utc::now();

        // 2 Pending
        for _ in 0..2 {
            insert_task_row(&pool, Uuid::new_v4(), job_id, "pending", 0, &["p"]).await;
        }

        // 1 Assigned with live InFlight deadline -> in_flight
        let live_task = Uuid::new_v4();
        insert_task_row(&pool, live_task, job_id, "assigned", 0, &["a"]).await;
        insert_assignment_row(
            &pool,
            Uuid::new_v4(),
            live_task,
            Uuid::new_v4(),
            now,
            now + Duration::seconds(60),
            "in_flight",
        )
        .await;

        // 1 Assigned with expired deadline -> NOT counted as in_flight
        let stale_task = Uuid::new_v4();
        insert_task_row(&pool, stale_task, job_id, "assigned", 0, &["a"]).await;
        insert_assignment_row(
            &pool,
            Uuid::new_v4(),
            stale_task,
            Uuid::new_v4(),
            now - Duration::seconds(120),
            now - Duration::seconds(60),
            "in_flight",
        )
        .await;

        // 3 Completed total, of which 2 are completed_by_me (assignments
        // submitted by `me`) and 1 by `other`.
        for submitter in [me, me, other] {
            let t = Uuid::new_v4();
            insert_task_row(&pool, t, job_id, "completed", 1, &["c"]).await;
            insert_assignment_row(
                &pool,
                Uuid::new_v4(),
                t,
                submitter,
                now - Duration::seconds(30),
                now + Duration::seconds(30),
                "submitted",
            )
            .await;
        }

        // Unrelated job — must NOT leak into our counts.
        let other_job = Uuid::new_v4();
        insert_job_row(&pool, other_job, "processing").await;
        let other_task = Uuid::new_v4();
        insert_task_row(&pool, other_task, other_job, "completed", 1, &["c"]).await;
        insert_assignment_row(
            &pool,
            Uuid::new_v4(),
            other_task,
            me,
            now,
            now + Duration::seconds(60),
            "submitted",
        )
        .await;

        let stats = get_task_stats(&pool, job_id, me).await.unwrap();
        assert_eq!(stats.pending, 2);
        assert_eq!(stats.in_flight, 1);
        assert_eq!(stats.completed_total, 3);
        assert_eq!(stats.completed_by_me, 2);
    }

    #[tokio::test]
    async fn test_job_exists_returns_true_and_false() {
        let Some((pool, _guard)) = pool_or_skip().await else {
            return;
        };

        let job_id = Uuid::new_v4();
        insert_job_row(&pool, job_id, "uploaded").await;
        assert!(job_exists(&pool, job_id).await.unwrap());

        assert!(!job_exists(&pool, Uuid::new_v4()).await.unwrap());
    }
}
