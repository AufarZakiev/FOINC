use chrono::{DateTime, Duration, Utc};
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use foinc_integrations::TaskStats;
use foinc_result_aggregation::{normalize_stdout, try_resolve_consensus};

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
///
/// `redundancy_target` is copied onto every inserted row and controls how
/// many distinct-worker `Submitted` assignments must accumulate before
/// consensus is attempted. The handler reads `FOINC_REDUNDANCY` and falls
/// back to 2 when the env var is unset or invalid.
pub async fn insert_pending_tasks(
    pool: &PgPool,
    job_id: Uuid,
    rows: &[String],
    redundancy_target: i16,
) -> Result<u32, sqlx::Error> {
    let mut tx = pool.begin().await?;

    for (idx, line) in rows.iter().enumerate() {
        let task_id = Uuid::new_v4();
        // `input_rows` is a Postgres TEXT[]; pass as a slice.
        let input_rows = vec![line.clone()];
        sqlx::query(
            r#"
            INSERT INTO tasks
                (task_id, job_id, chunk_index, input_rows, status, attempts,
                 redundancy_target, created_at)
            VALUES
                ($1, $2, $3, $4, 'pending', 0, $5, now())
            "#,
        )
        .bind(task_id)
        .bind(job_id)
        .bind(idx as i32)
        .bind(&input_rows)
        .bind(redundancy_target)
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
/// Phase-4 candidate rule (see spec):
/// - `status IN ('pending', 'awaiting_consensus')`, AND
/// - effective dispatches (Submitted + live InFlight) < `redundancy_target`, AND
/// - caller has no prior assignment (any status) for the task.
///
/// Concurrency: `SELECT ... FOR UPDATE SKIP LOCKED` on the `tasks` row
/// serializes pickers and serializes timeout reclamation against new
/// dispatch. Expired InFlight assignments are reaped inline; a reclaimed
/// task whose `attempts` has hit the cap is marked Failed and the loop
/// moves on.
pub async fn pick_next_task(
    pool: &PgPool,
    worker_id: Uuid,
) -> Result<Option<PickedTask>, sqlx::Error> {
    loop {
        let mut tx: Transaction<'_, Postgres> = pool.begin().await?;

        // Candidate = Pending / AwaitingConsensus task where:
        //   (Submitted + live InFlight) < redundancy_target, AND
        //   no assignment exists for (task, worker_id).
        // Expired InFlight rows are also surfaced via `expired_assignment_id`
        // so the loop can reclaim them on a candidate already returned by
        // the filter. Order by `created_at ASC` to prefer older work.
        let row = sqlx::query_as::<_, CandidateRow>(
            r#"
            SELECT t.task_id, t.job_id, t.status, t.attempts, t.input_rows,
                   t.redundancy_target,
                   expired.assignment_id AS expired_assignment_id
              FROM tasks t
         LEFT JOIN LATERAL (
                SELECT assignment_id, deadline_at
                  FROM assignments
                 WHERE task_id = t.task_id
                   AND status = 'in_flight'
                   AND deadline_at < now()
                 ORDER BY assigned_at DESC
                 LIMIT 1
              ) expired ON TRUE
             WHERE t.status IN ('pending', 'awaiting_consensus')
               AND (
                    SELECT COUNT(*) FROM assignments a
                     WHERE a.task_id = t.task_id
                       AND (
                            a.status = 'submitted'
                         OR (a.status = 'in_flight' AND a.deadline_at >= now())
                       )
                   ) < t.redundancy_target
               AND NOT EXISTS (
                    SELECT 1 FROM assignments a
                     WHERE a.task_id = t.task_id
                       AND a.worker_id = $1
               )
             ORDER BY t.created_at ASC
             FOR UPDATE OF t SKIP LOCKED
             LIMIT 1
            "#,
        )
        .bind(worker_id)
        .fetch_optional(&mut *tx)
        .await?;

        let Some(cand) = row else {
            tx.commit().await?;
            return Ok(None);
        };

        // Reclamation path: there's an expired InFlight assignment on this
        // candidate. Attempts-cap check runs BEFORE any mutation.
        if let Some(old_assignment) = cand.expired_assignment_id {
            if cand.attempts >= MAX_ATTEMPTS {
                sqlx::query(
                    r#"UPDATE assignments SET status = 'timed_out' WHERE assignment_id = $1"#,
                )
                .bind(old_assignment)
                .execute(&mut *tx)
                .await?;
                sqlx::query(r#"UPDATE tasks SET status = 'failed' WHERE task_id = $1"#)
                    .bind(cand.task_id)
                    .execute(&mut *tx)
                    .await?;
                tx.commit().await?;
                continue;
            }

            sqlx::query(
                r#"UPDATE assignments SET status = 'timed_out' WHERE assignment_id = $1"#,
            )
            .bind(old_assignment)
            .execute(&mut *tx)
            .await?;
            sqlx::query(
                r#"UPDATE tasks SET attempts = attempts + 1 WHERE task_id = $1"#,
            )
            .bind(cand.task_id)
            .execute(&mut *tx)
            .await?;
        }

        // Re-evaluate under the row lock: has another writer pushed the
        // task over `redundancy_target` since we picked it? Or is the
        // caller now already assigned to it? If so, release the lock and
        // loop for another candidate.
        let effective_dispatches: i64 = sqlx::query_scalar(
            r#"
            SELECT COUNT(*)
              FROM assignments
             WHERE task_id = $1
               AND (
                    status = 'submitted'
                 OR (status = 'in_flight' AND deadline_at >= now())
               )
            "#,
        )
        .bind(cand.task_id)
        .fetch_one(&mut *tx)
        .await?;

        if effective_dispatches >= cand.redundancy_target as i64 {
            tx.commit().await?;
            continue;
        }

        let already_assigned: Option<Uuid> = sqlx::query_scalar(
            r#"
            SELECT assignment_id FROM assignments
             WHERE task_id = $1 AND worker_id = $2
             LIMIT 1
            "#,
        )
        .bind(cand.task_id)
        .bind(worker_id)
        .fetch_optional(&mut *tx)
        .await?;
        if already_assigned.is_some() {
            tx.commit().await?;
            continue;
        }

        // Insert new InFlight assignment. Flip status Pending -> Assigned;
        // leave AwaitingConsensus unchanged per the Phase-4 state machine.
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

        if cand.status == "pending" {
            sqlx::query(r#"UPDATE tasks SET status = 'assigned' WHERE task_id = $1"#)
                .bind(cand.task_id)
                .execute(&mut *tx)
                .await?;
        }

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
    redundancy_target: i16,
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

/// Outcome of a sibling-terminality recompute. Returned by `fail_task` and
/// used by orphan-recovery callers that want to know whether their action
/// ended the job. Defined at module scope so it can be shared across db
/// helpers; kept distinct from `JobTerminal` to preserve `submit_task`'s
/// existing API shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobTerminalState {
    /// Siblings are not all terminal yet; job is still `processing`.
    StillProcessing,
    /// All siblings terminal, ≥1 Completed — job flipped to `completed`.
    FlippedCompleted,
    /// All siblings terminal, none Completed — job flipped to `failed`.
    FlippedFailed,
}

/// Persist a submission for the task, apply consensus, and recompute job
/// terminality.
///
/// Phase-4 flow (all in a single transaction):
/// 1. `SELECT ... FOR UPDATE` the caller's most recent InFlight assignment;
///    verify worker_id and deadline; mark `Submitted` with
///    stdout/stderr/duration_ms and the SHA-256 of `normalize_stdout`.
/// 2. Count `Submitted` assignments for this task.
///    - If `count < redundancy_target`: set `tasks.status = 'awaiting_consensus'`
///      (idempotent). Skip the consensus hook.
///    - Else: call `result_aggregation::try_resolve_consensus(tx, task_id)`.
/// 3. Recompute job-level terminality using the tightened Phase-4 rule:
///    - `completed` iff every sibling is `Completed`.
///    - `failed` iff ≥1 sibling `Failed` and every sibling terminal.
pub async fn submit_task(
    pool: &PgPool,
    task_id: Uuid,
    worker_id: Uuid,
    stdout: &str,
    stderr: &str,
    duration_ms: f64,
) -> Result<SubmitOutcome, sqlx::Error> {
    let mut tx: Transaction<'_, Postgres> = pool.begin().await?;

    // Lock the task row first so we can read `redundancy_target` under the
    // same lock we'll later update `tasks.status` through.
    let task_row = sqlx::query_as::<_, TaskHeader>(
        r#"SELECT task_id, job_id, redundancy_target FROM tasks WHERE task_id = $1 FOR UPDATE"#,
    )
    .bind(task_id)
    .fetch_optional(&mut *tx)
    .await?;

    let Some(task) = task_row else {
        tx.rollback().await?;
        return Ok(SubmitOutcome::NotFound);
    };

    // Lock the caller's most recent InFlight assignment row for this task.
    //
    // Filtering by `worker_id` and `status = 'in_flight'` before the LIMIT
    // is load-bearing under redundancy: with two or more concurrent
    // in-flight assignments on the same task, an unfiltered "most recent by
    // assigned_at" could return a sibling worker's row and cause the
    // legitimate submitter to be rejected as a conflict. Picking the
    // caller's own row here ensures each worker's submission is judged
    // against its own assignment.
    let current = sqlx::query_as::<_, CurrentAssignmentRow>(
        r#"
        SELECT assignment_id, deadline_at
          FROM assignments
         WHERE task_id = $1
           AND worker_id = $2
           AND status = 'in_flight'
         ORDER BY assigned_at DESC
         LIMIT 1
         FOR UPDATE
        "#,
    )
    .bind(task_id)
    .bind(worker_id)
    .fetch_optional(&mut *tx)
    .await?;

    let Some(curr) = current else {
        // The submitter has no in-flight assignment for this task. Either
        // they never held one, it already transitioned out of `in_flight`,
        // or the task simply has no assignments at all. Report Conflict;
        // the handler maps this to 409 with "No in-flight assignment for
        // this worker".
        tx.rollback().await?;
        return Ok(SubmitOutcome::Conflict);
    };

    // Validate under the row lock. The SQL filter above already pins
    // worker_id and status; the remaining check is the deadline.
    if curr.deadline_at < Utc::now() {
        tx.rollback().await?;
        return Ok(SubmitOutcome::Conflict);
    }

    // Compute the result hash over normalized stdout. Both the
    // normalization rule and the hash-hex encoding must stay in sync with
    // the consensus comparator — the single source of truth is
    // `foinc_result_aggregation::normalize_stdout`.
    let normalized = normalize_stdout(stdout);
    let mut hasher = Sha256::new();
    hasher.update(normalized.as_bytes());
    let result_hash: String = format!("{:x}", hasher.finalize());

    // Accept the submission and persist the result hash.
    sqlx::query(
        r#"
        UPDATE assignments
           SET status = 'submitted',
               stdout = $2,
               stderr = $3,
               duration_ms = $4,
               result_hash = $5
         WHERE assignment_id = $1
        "#,
    )
    .bind(curr.assignment_id)
    .bind(stdout)
    .bind(stderr)
    .bind(duration_ms)
    .bind(&result_hash)
    .execute(&mut *tx)
    .await?;

    // Count submitted assignments now that this one is in.
    let submitted_count: i64 = sqlx::query_scalar(
        r#"SELECT COUNT(*) FROM assignments WHERE task_id = $1 AND status = 'submitted'"#,
    )
    .bind(task_id)
    .fetch_one(&mut *tx)
    .await?;

    if submitted_count < task.redundancy_target as i64 {
        // Not enough submissions yet — park the task in awaiting_consensus.
        // The UPDATE is a no-op if the status is already awaiting_consensus
        // (idempotent under the row lock).
        sqlx::query(
            r#"UPDATE tasks SET status = 'awaiting_consensus' WHERE task_id = $1 AND status <> 'awaiting_consensus'"#,
        )
        .bind(task_id)
        .execute(&mut *tx)
        .await?;
    } else {
        // Enough submissions — hand off to the consensus policy. The hook
        // runs inside our tx and may update tasks.status,
        // winning_assignment_id, or redundancy_target.
        let _outcome = try_resolve_consensus(&mut tx, task_id).await?;
    }

    // Recompute job-level terminality using the Phase-4 rule. Read each
    // sibling's status from within the same transaction.
    let job_terminal =
        recompute_job_terminality(&mut tx, task.job_id).await?;

    tx.commit().await?;

    Ok(SubmitOutcome::Submitted { job_terminal })
}

#[derive(sqlx::FromRow)]
struct TaskHeader {
    #[allow(dead_code)]
    task_id: Uuid,
    job_id: Uuid,
    redundancy_target: i16,
}

#[derive(sqlx::FromRow)]
struct CurrentAssignmentRow {
    assignment_id: Uuid,
    deadline_at: DateTime<Utc>,
}

#[derive(sqlx::FromRow)]
struct SiblingCounts {
    non_terminal: i64,
    completed: i64,
    failed: i64,
}

/// Apply the Phase-4 job-terminality rule inside an existing transaction
/// and return the resulting [`JobTerminal`] if the job flipped.
///
/// Rule (tightened from Phase 3):
/// - `completed` iff every sibling is `Completed`.
/// - `failed` iff at least one sibling is `Failed` AND every sibling is
///   terminal.
/// - otherwise the job stays `processing`.
///
/// The CAS on `UPDATE jobs ... WHERE status='processing'` keeps the flip
/// idempotent — a concurrent writer that already flipped the job is a
/// no-op here.
async fn recompute_job_terminality(
    tx: &mut Transaction<'_, Postgres>,
    job_id: Uuid,
) -> Result<Option<JobTerminal>, sqlx::Error> {
    let counts = sqlx::query_as::<_, SiblingCounts>(
        r#"
        SELECT
            COUNT(*) FILTER (WHERE status NOT IN ('completed', 'failed')) AS non_terminal,
            COUNT(*) FILTER (WHERE status = 'completed') AS completed,
            COUNT(*) FILTER (WHERE status = 'failed') AS failed
          FROM tasks
         WHERE job_id = $1
        "#,
    )
    .bind(job_id)
    .fetch_one(&mut **tx)
    .await?;

    if counts.non_terminal > 0 {
        return Ok(None);
    }

    // All siblings terminal. Phase-4 rule:
    //   - any Failed  → job failed
    //   - all Completed → job completed
    let outcome = if counts.failed > 0 {
        sqlx::query(
            r#"UPDATE jobs SET status = 'failed' WHERE job_id = $1 AND status = 'processing'"#,
        )
        .bind(job_id)
        .execute(&mut **tx)
        .await?;
        JobTerminal::Failed
    } else if counts.completed > 0 {
        sqlx::query(
            r#"UPDATE jobs SET status = 'completed' WHERE job_id = $1 AND status = 'processing'"#,
        )
        .bind(job_id)
        .execute(&mut **tx)
        .await?;
        JobTerminal::Completed
    } else {
        // No tasks at all. Nothing to flip; stay processing.
        return Ok(None);
    };

    Ok(Some(outcome))
}

/// Mark an orphaned task `Failed`.
///
/// Used by the orphan-recovery path in `POST /tasks/next` when the job's
/// script or CSV cannot be read from disk. In a single transaction:
///
/// 1. Look up the task's parent `job_id` (needed for the sibling recompute).
/// 2. Mark the current in-flight assignment `TimedOut`.
/// 3. Mark the task `Failed`. `attempts` is NOT incremented — orphaned
///    files are unrecoverable, not a retry.
/// 4. Recompute sibling terminality using the same rule as `submit_task`:
///    if every sibling is terminal (`Completed` or `Failed`), flip the job
///    to `completed` (≥1 `Completed`) or `failed` (all `Failed`).
///
/// The returned [`JobTerminalState`] tells the caller whether the job
/// flipped, so the handler can log or emit a signal. The CAS on
/// `UPDATE jobs ... WHERE status='processing'` mirrors `submit_task` and
/// keeps this path idempotent if another writer already flipped the job.
pub async fn fail_task(
    pool: &PgPool,
    task_id: Uuid,
) -> Result<JobTerminalState, sqlx::Error> {
    let mut tx: Transaction<'_, Postgres> = pool.begin().await?;

    // 1. Look up parent job_id.
    let job_id: Uuid =
        sqlx::query_scalar(r#"SELECT job_id FROM tasks WHERE task_id = $1"#)
            .bind(task_id)
            .fetch_one(&mut *tx)
            .await?;

    // 2. Mark the current in-flight assignment TimedOut. We target the most
    //    recent in-flight row by `assigned_at DESC` — matches the locking
    //    order used by `submit_task`. If there is no in-flight assignment
    //    (defensive: task is assigned but the row went missing somehow), we
    //    still proceed to fail the task.
    sqlx::query(
        r#"
        UPDATE assignments
           SET status = 'timed_out'
         WHERE assignment_id = (
                SELECT assignment_id
                  FROM assignments
                 WHERE task_id = $1
                   AND status = 'in_flight'
                 ORDER BY assigned_at DESC
                 LIMIT 1
         )
        "#,
    )
    .bind(task_id)
    .execute(&mut *tx)
    .await?;

    // 3. Fail the task. Do NOT touch `attempts`.
    sqlx::query(r#"UPDATE tasks SET status = 'failed' WHERE task_id = $1"#)
        .bind(task_id)
        .execute(&mut *tx)
        .await?;

    // 4. Recompute sibling terminality with the Phase-4 rule:
    //    - completed iff every sibling is Completed,
    //    - failed iff ≥1 Failed and every sibling terminal.
    //    Since we just marked this task Failed, the presence-of-Failed
    //    condition is guaranteed when every sibling is terminal.
    let sibling_counts = sqlx::query_as::<_, SiblingCounts>(
        r#"
        SELECT
            COUNT(*) FILTER (WHERE status NOT IN ('completed', 'failed')) AS non_terminal,
            COUNT(*) FILTER (WHERE status = 'completed') AS completed,
            COUNT(*) FILTER (WHERE status = 'failed') AS failed
          FROM tasks
         WHERE job_id = $1
        "#,
    )
    .bind(job_id)
    .fetch_one(&mut *tx)
    .await?;

    let outcome = if sibling_counts.non_terminal == 0 {
        if sibling_counts.failed > 0 {
            sqlx::query(
                r#"UPDATE jobs SET status = 'failed' WHERE job_id = $1 AND status = 'processing'"#,
            )
            .bind(job_id)
            .execute(&mut *tx)
            .await?;
            JobTerminalState::FlippedFailed
        } else if sibling_counts.completed > 0 {
            sqlx::query(
                r#"UPDATE jobs SET status = 'completed' WHERE job_id = $1 AND status = 'processing'"#,
            )
            .bind(job_id)
            .execute(&mut *tx)
            .await?;
            JobTerminalState::FlippedCompleted
        } else {
            JobTerminalState::StillProcessing
        }
    } else {
        JobTerminalState::StillProcessing
    };

    tx.commit().await?;

    Ok(outcome)
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

    // Phase-4 in_flight: any task in Assigned OR AwaitingConsensus with at
    // least one InFlight assignment whose deadline_at is still in the
    // future. `EXISTS` matches the spec's "at least one" wording and
    // avoids double-counting tasks that have several stale InFlight rows.
    let in_flight: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(*)
          FROM tasks t
         WHERE t.job_id = $1
           AND t.status IN ('assigned', 'awaiting_consensus')
           AND EXISTS (
                SELECT 1 FROM assignments a
                 WHERE a.task_id = t.task_id
                   AND a.status = 'in_flight'
                   AND a.deadline_at >= now()
           )
        "#,
    )
    .bind(job_id)
    .fetch_one(pool)
    .await?;

    let awaiting_consensus: i64 = sqlx::query_scalar(
        r#"SELECT COUNT(*) FROM tasks WHERE job_id = $1 AND status = 'awaiting_consensus'"#,
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
        awaiting_consensus,
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
        // Phase-4 signature takes `redundancy_target`; pass the default 2.
        let n = insert_pending_tasks(&pool, job_id, &rows, 2).await.unwrap();
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
        // Phase-4: candidate pool is `status IN ('pending', 'awaiting_consensus')`
        // — an `assigned` task would not be re-picked. Use
        // `awaiting_consensus` to model a task whose previous dispatch expired
        // without a submission and is back in the candidate pool for
        // reclamation.
        insert_task_row(&pool, task_id, job_id, "awaiting_consensus", 1, &["x,y"]).await;

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

        // Phase-4: only `pending` → `assigned` on pick; `awaiting_consensus`
        // stays as-is. attempts must still be bumped by the reclamation path.
        let (status, attempts) = get_task_status_and_attempts(&pool, task_id).await;
        assert_eq!(status, "awaiting_consensus");
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
        // Phase-4: only `pending` / `awaiting_consensus` are picker candidates,
        // so seed as `awaiting_consensus` to hit the reclamation path.
        insert_task_row(&pool, task_id, job_id, "awaiting_consensus", MAX_ATTEMPTS, &["exhausted"]).await;

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
        // Phase-4: default `redundancy_target = 2` leaves a single submission
        // in `awaiting_consensus`. The happy path asserts Completed, so seed
        // with `redundancy_target = 1` — one submission then collapses to
        // Completed via consensus's strict-majority rule
        // (`2 * majority.count > submission_count` → `2 * 1 > 1`).
        insert_task_row_rt(&pool, task_id, job_id, "assigned", 0, &["a,b"], 1).await;

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
        // Phase-4 rule: job flips to `completed` iff ALL sibling tasks are
        // `Completed`. Seed two tasks in the same job with
        // `redundancy_target = 1` so each single submission individually
        // collapses to Completed. After the first submit the sibling is
        // still Assigned, so the job stays in `processing`. After the
        // second submit every sibling is Completed -> job flips to
        // `completed`.
        let Some((pool, _guard)) = pool_or_skip().await else {
            return;
        };

        let job_id = Uuid::new_v4();
        insert_job_row(&pool, job_id, "processing").await;

        // First sibling: single-submission Completed target.
        let first_task = Uuid::new_v4();
        insert_task_row_rt(&pool, first_task, job_id, "assigned", 0, &["x"], 1).await;
        let first_assignment = Uuid::new_v4();
        let first_worker = Uuid::new_v4();
        let now = Utc::now();
        insert_assignment_row(
            &pool,
            first_assignment,
            first_task,
            first_worker,
            now,
            now + Duration::seconds(60),
            "in_flight",
        )
        .await;

        // Second sibling: also redundancy_target = 1 so its single submit
        // closes the task out.
        let second_task = Uuid::new_v4();
        insert_task_row_rt(&pool, second_task, job_id, "assigned", 0, &["z"], 1).await;
        let second_assignment = Uuid::new_v4();
        let second_worker = Uuid::new_v4();
        insert_assignment_row(
            &pool,
            second_assignment,
            second_task,
            second_worker,
            now,
            now + Duration::seconds(60),
            "in_flight",
        )
        .await;

        // First submit: the other sibling is still Assigned -> job stays
        // `processing`.
        let first_outcome = submit_task(&pool, first_task, first_worker, "ok1", "", 1.0)
            .await
            .unwrap();
        assert_eq!(
            first_outcome,
            SubmitOutcome::Submitted { job_terminal: None }
        );
        assert_eq!(get_job_status(&pool, job_id).await, "processing");

        // Second submit: every sibling is now Completed -> Phase-4 rule
        // flips the job to `completed`.
        let second_outcome =
            submit_task(&pool, second_task, second_worker, "ok2", "", 2.0)
                .await
                .unwrap();
        assert_eq!(
            second_outcome,
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
        // Phase-4: picker candidates are `pending` / `awaiting_consensus`
        // only. Seed as `awaiting_consensus` so pick_next_task reaches the
        // reclamation branch that flips the task to `failed`.
        insert_task_row(&pool, dying_task, job_id, "awaiting_consensus", MAX_ATTEMPTS, &["y"]).await;
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

    // -------------------------------------------------------------------
    // fail_task
    // -------------------------------------------------------------------

    #[tokio::test]
    async fn test_fail_task_marks_task_failed_and_assignment_timed_out() {
        // Orphan-recovery path: a task with a live InFlight assignment is
        // failed. The assignment must transition InFlight -> TimedOut in the
        // same tx that flips the task to Failed.
        let Some((pool, _guard)) = pool_or_skip().await else {
            return;
        };

        let job_id = Uuid::new_v4();
        insert_job_row(&pool, job_id, "processing").await;

        let task_id = Uuid::new_v4();
        insert_task_row(&pool, task_id, job_id, "assigned", 1, &["orphan"]).await;

        let assignment_id = Uuid::new_v4();
        let now = Utc::now();
        insert_assignment_row(
            &pool,
            assignment_id,
            task_id,
            Uuid::new_v4(),
            now,
            now + Duration::seconds(60),
            "in_flight",
        )
        .await;

        let outcome = fail_task(&pool, task_id).await.unwrap();
        // Single task, no completed siblings -> job flipped to failed.
        assert_eq!(outcome, JobTerminalState::FlippedFailed);

        let (status, _) = get_task_status_and_attempts(&pool, task_id).await;
        assert_eq!(status, "failed");
        assert_eq!(get_assignment_status(&pool, assignment_id).await, "timed_out");
    }

    #[tokio::test]
    async fn test_fail_task_does_not_increment_attempts() {
        // Orphaned files are unrecoverable and must NOT count as a retry.
        let Some((pool, _guard)) = pool_or_skip().await else {
            return;
        };

        let job_id = Uuid::new_v4();
        insert_job_row(&pool, job_id, "processing").await;

        let task_id = Uuid::new_v4();
        // Pre-set attempts=3 so we can observe it stays at 3 after fail_task.
        insert_task_row(&pool, task_id, job_id, "assigned", 3, &["orphan"]).await;

        let assignment_id = Uuid::new_v4();
        let now = Utc::now();
        insert_assignment_row(
            &pool,
            assignment_id,
            task_id,
            Uuid::new_v4(),
            now,
            now + Duration::seconds(60),
            "in_flight",
        )
        .await;

        let _ = fail_task(&pool, task_id).await.unwrap();

        let (status, attempts) = get_task_status_and_attempts(&pool, task_id).await;
        assert_eq!(status, "failed");
        assert_eq!(attempts, 3, "fail_task must not touch attempts");
    }

    #[tokio::test]
    async fn test_fail_task_returns_still_processing_when_siblings_outstanding() {
        // Two tasks in the job. Failing one still leaves a Pending sibling,
        // so the job stays in `processing`.
        let Some((pool, _guard)) = pool_or_skip().await else {
            return;
        };

        let job_id = Uuid::new_v4();
        insert_job_row(&pool, job_id, "processing").await;

        // Sibling remains Pending — job must stay Processing.
        let pending_sibling = Uuid::new_v4();
        insert_task_row(&pool, pending_sibling, job_id, "pending", 0, &["live"]).await;

        // The task we orphan-fail.
        let task_id = Uuid::new_v4();
        insert_task_row(&pool, task_id, job_id, "assigned", 0, &["orphan"]).await;
        let assignment_id = Uuid::new_v4();
        let now = Utc::now();
        insert_assignment_row(
            &pool,
            assignment_id,
            task_id,
            Uuid::new_v4(),
            now,
            now + Duration::seconds(60),
            "in_flight",
        )
        .await;

        let outcome = fail_task(&pool, task_id).await.unwrap();
        assert_eq!(outcome, JobTerminalState::StillProcessing);

        assert_eq!(get_task_status_and_attempts(&pool, task_id).await.0, "failed");
        assert_eq!(get_assignment_status(&pool, assignment_id).await, "timed_out");
        assert_eq!(get_job_status(&pool, job_id).await, "processing");
    }

    #[tokio::test]
    async fn test_fail_task_flips_job_to_failed_when_all_failed() {
        // Two tasks, both orphaned in sequence. Last call flips job to failed.
        let Some((pool, _guard)) = pool_or_skip().await else {
            return;
        };

        let job_id = Uuid::new_v4();
        insert_job_row(&pool, job_id, "processing").await;

        let t1 = Uuid::new_v4();
        insert_task_row(&pool, t1, job_id, "assigned", 0, &["a"]).await;
        let a1 = Uuid::new_v4();
        let now = Utc::now();
        insert_assignment_row(
            &pool,
            a1,
            t1,
            Uuid::new_v4(),
            now,
            now + Duration::seconds(60),
            "in_flight",
        )
        .await;

        let t2 = Uuid::new_v4();
        insert_task_row(&pool, t2, job_id, "assigned", 0, &["b"]).await;
        let a2 = Uuid::new_v4();
        insert_assignment_row(
            &pool,
            a2,
            t2,
            Uuid::new_v4(),
            now,
            now + Duration::seconds(60),
            "in_flight",
        )
        .await;

        // First fail: sibling still Assigned -> StillProcessing.
        let out1 = fail_task(&pool, t1).await.unwrap();
        assert_eq!(out1, JobTerminalState::StillProcessing);
        assert_eq!(get_job_status(&pool, job_id).await, "processing");

        // Second fail: all siblings terminal, none Completed -> FlippedFailed.
        let out2 = fail_task(&pool, t2).await.unwrap();
        assert_eq!(out2, JobTerminalState::FlippedFailed);
        assert_eq!(get_job_status(&pool, job_id).await, "failed");

        assert_eq!(get_assignment_status(&pool, a1).await, "timed_out");
        assert_eq!(get_assignment_status(&pool, a2).await, "timed_out");
    }

    // -------------------------------------------------------------------
    // Phase 4: redundancy, consensus, and tightened job-terminality
    // -------------------------------------------------------------------
    //
    // Note: The pre-Phase-4 test
    // `test_fail_task_flips_job_to_completed_when_last_sibling_completed`
    // was removed. Under the Phase-4 rule (any Failed sibling + all
    // terminal -> job Failed), a job with one Completed + one Failed
    // sibling MUST flip to Failed, not Completed. That tightened rule is
    // covered by `test_fail_task_terminality_tightened_any_failed_flips_job_to_failed`.

    /// Insert a task row with an explicit `redundancy_target`. The default
    /// helper relies on the Postgres column default (`2`) which is fine for
    /// most Phase 3 tests, but Phase 4 cases need to drive target = 1, 3,
    /// etc. explicitly.
    async fn insert_task_row_rt(
        pool: &PgPool,
        task_id: Uuid,
        job_id: Uuid,
        status: &str,
        attempts: i32,
        input_rows: &[&str],
        redundancy_target: i16,
    ) {
        let rows: Vec<String> = input_rows.iter().map(|s| s.to_string()).collect();
        sqlx::query(
            r#"
            INSERT INTO tasks
                (task_id, job_id, chunk_index, input_rows, status, attempts,
                 redundancy_target, created_at)
            VALUES
                ($1, $2, 0, $3, $4, $5, $6, now())
            "#,
        )
        .bind(task_id)
        .bind(job_id)
        .bind(&rows)
        .bind(status)
        .bind(attempts)
        .bind(redundancy_target)
        .execute(pool)
        .await
        .unwrap();
    }

    /// Insert an assignment with a fully explicit stdout/stderr/duration
    /// /result_hash payload, so consensus fixtures can be seeded without
    /// going through `submit_task`.
    #[allow(clippy::too_many_arguments)]
    async fn insert_submitted_assignment(
        pool: &PgPool,
        assignment_id: Uuid,
        task_id: Uuid,
        worker_id: Uuid,
        assigned_at: chrono::DateTime<Utc>,
        deadline_at: chrono::DateTime<Utc>,
        stdout: &str,
        result_hash: Option<&str>,
    ) {
        sqlx::query(
            r#"
            INSERT INTO assignments
                (assignment_id, task_id, worker_id, assigned_at, deadline_at,
                 status, stdout, stderr, duration_ms, result_hash)
            VALUES
                ($1, $2, $3, $4, $5, 'submitted', $6, '', 1.0, $7)
            "#,
        )
        .bind(assignment_id)
        .bind(task_id)
        .bind(worker_id)
        .bind(assigned_at)
        .bind(deadline_at)
        .bind(stdout)
        .bind(result_hash)
        .execute(pool)
        .await
        .unwrap();
    }

    /// Fetch `tasks.winning_assignment_id` for a task.
    async fn get_winning_assignment_id(pool: &PgPool, task_id: Uuid) -> Option<Uuid> {
        sqlx::query_scalar::<_, Option<Uuid>>(
            r#"SELECT winning_assignment_id FROM tasks WHERE task_id = $1"#,
        )
        .bind(task_id)
        .fetch_one(pool)
        .await
        .unwrap()
    }

    /// Fetch an assignment's `result_hash`.
    async fn get_assignment_result_hash(pool: &PgPool, assignment_id: Uuid) -> Option<String> {
        sqlx::query_scalar::<_, Option<String>>(
            r#"SELECT result_hash FROM assignments WHERE assignment_id = $1"#,
        )
        .bind(assignment_id)
        .fetch_one(pool)
        .await
        .unwrap()
    }

    /// Compute the expected SHA-256 hex of `normalize_stdout(stdout)` using
    /// the exact normalization + hashing pipeline used inside `submit_task`.
    fn expected_result_hash(stdout: &str) -> String {
        let normalized = foinc_result_aggregation::normalize_stdout(stdout);
        let mut hasher = Sha256::new();
        hasher.update(normalized.as_bytes());
        format!("{:x}", hasher.finalize())
    }

    #[tokio::test]
    async fn test_pick_next_task_respects_redundancy_target() {
        // One task with `redundancy_target = 2`. The candidate predicate is
        // `status IN ('pending', 'awaiting_consensus')`, so we must drive
        // the task through those states to exercise redundant dispatches.
        //
        // Flow:
        //   1. Insert 1 Pending task, redundancy_target = 2.
        //   2. Worker A picks -> task flips to `assigned`.
        //   3. A submits -> submissions=1 < target=2, task flips to
        //      `awaiting_consensus`.
        //   4. Worker B picks -> gets the SAME task (candidate again because
        //      status = awaiting_consensus, effective dispatches = 1 < 2).
        //   5. Worker C picks -> None (after B's pick, effective dispatches
        //      = 2 = target).
        let Some((pool, _guard)) = pool_or_skip().await else {
            return;
        };

        let job_id = Uuid::new_v4();
        insert_job_row(&pool, job_id, "processing").await;
        let task_id = Uuid::new_v4();
        insert_task_row_rt(&pool, task_id, job_id, "pending", 0, &["r"], 2).await;

        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let c = Uuid::new_v4();

        // Step 2: worker A picks the Pending task.
        let first = pick_next_task(&pool, a).await.unwrap();
        let first = first.expect("worker A gets the task");
        assert_eq!(first.task_id, task_id);
        // After A's pick the task must be `assigned`.
        assert_eq!(get_task_status_and_attempts(&pool, task_id).await.0, "assigned");

        // Step 3: A submits. Submissions=1 < target=2 -> task goes to
        // `awaiting_consensus` (back on the candidate list for other workers).
        let a_assignment_id = sqlx::query_scalar::<_, Uuid>(
            r#"SELECT assignment_id FROM assignments WHERE task_id = $1 AND worker_id = $2"#,
        )
        .bind(task_id)
        .bind(a)
        .fetch_one(&pool)
        .await
        .unwrap();
        let outcome = submit_task(&pool, task_id, a, "hello", "", 1.0).await.unwrap();
        assert_eq!(outcome, SubmitOutcome::Submitted { job_terminal: None });
        assert_eq!(
            get_task_status_and_attempts(&pool, task_id).await.0,
            "awaiting_consensus"
        );
        assert_eq!(get_assignment_status(&pool, a_assignment_id).await, "submitted");

        // Step 4: worker B picks the same task (still a candidate:
        // awaiting_consensus + 1 effective dispatch < 2).
        let second = pick_next_task(&pool, b).await.unwrap();
        let second = second.expect("worker B also gets the task");
        assert_eq!(second.task_id, task_id);
        // Status stays awaiting_consensus (only Pending -> Assigned flips).
        assert_eq!(
            get_task_status_and_attempts(&pool, task_id).await.0,
            "awaiting_consensus"
        );

        // Step 5: worker C must see no candidates — effective dispatches
        // (1 submitted + 1 live in_flight) = 2 = target.
        let third = pick_next_task(&pool, c).await.unwrap();
        assert!(third.is_none(), "worker C must not receive any dispatch");
    }

    #[tokio::test]
    async fn test_pick_next_task_prevents_double_dispatch_to_same_worker() {
        // One Pending task with redundancy_target = 2. Worker A picks it
        // once; a second pick from A must return None even though there's
        // still a redundancy slot open.
        let Some((pool, _guard)) = pool_or_skip().await else {
            return;
        };

        let job_id = Uuid::new_v4();
        insert_job_row(&pool, job_id, "processing").await;
        let task_id = Uuid::new_v4();
        insert_task_row_rt(&pool, task_id, job_id, "pending", 0, &["r"], 2).await;

        let a = Uuid::new_v4();

        let first = pick_next_task(&pool, a).await.unwrap();
        assert!(first.is_some(), "first pick should succeed");

        // Slot 2 is still open, but not to worker A.
        let second = pick_next_task(&pool, a).await.unwrap();
        assert!(
            second.is_none(),
            "worker A must not be dispatched the same task twice"
        );
    }

    #[tokio::test]
    async fn test_pick_next_task_picks_awaiting_consensus_task() {
        // Seed a task in AwaitingConsensus with one Submitted assignment
        // (worker A) and redundancy_target = 2. Worker B should get the
        // task because effective dispatches (1 submitted) < target.
        let Some((pool, _guard)) = pool_or_skip().await else {
            return;
        };

        let job_id = Uuid::new_v4();
        insert_job_row(&pool, job_id, "processing").await;
        let task_id = Uuid::new_v4();
        insert_task_row_rt(
            &pool,
            task_id,
            job_id,
            "awaiting_consensus",
            0,
            &["r"],
            2,
        )
        .await;

        let a = Uuid::new_v4();
        let now = Utc::now();
        insert_submitted_assignment(
            &pool,
            Uuid::new_v4(),
            task_id,
            a,
            now - Duration::seconds(30),
            now + Duration::seconds(30),
            "hello",
            Some("deadbeef"),
        )
        .await;

        let b = Uuid::new_v4();
        let picked = pick_next_task(&pool, b).await.unwrap();
        let picked = picked.expect("worker B should pick the AwaitingConsensus task");
        assert_eq!(picked.task_id, task_id);

        // Status must STAY `awaiting_consensus` — only Pending flips to
        // Assigned at dispatch time.
        assert_eq!(
            get_task_status_and_attempts(&pool, task_id).await.0,
            "awaiting_consensus"
        );
    }

    #[tokio::test]
    async fn test_submit_task_stores_result_hash() {
        // Submit a task via the real helper and assert `assignments.result_hash`
        // equals the hex SHA-256 of `normalize_stdout(stdout)`.
        let Some((pool, _guard)) = pool_or_skip().await else {
            return;
        };

        let job_id = Uuid::new_v4();
        insert_job_row(&pool, job_id, "processing").await;
        let task_id = Uuid::new_v4();
        // redundancy_target = 1 so a single submission closes consensus and
        // the assignment stays written — but the hash field is populated
        // regardless of which branch submit_task takes.
        insert_task_row_rt(&pool, task_id, job_id, "assigned", 0, &["a"], 1).await;

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

        // Include trailing whitespace + blank lines so normalization is
        // observable, not just a no-op.
        let stdout = "hello world   \n\n";
        let outcome = submit_task(&pool, task_id, worker_id, stdout, "", 1.0)
            .await
            .unwrap();
        assert!(matches!(outcome, SubmitOutcome::Submitted { .. }));

        let got = get_assignment_result_hash(&pool, assignment_id)
            .await
            .expect("result_hash must be populated on submit");
        assert_eq!(got, expected_result_hash(stdout));
        // Sanity: 64 hex chars.
        assert_eq!(got.len(), 64);
        assert!(got.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[tokio::test]
    async fn test_submit_task_transitions_to_awaiting_consensus_when_below_target() {
        // redundancy_target = 3; seed one InFlight assignment. After submit,
        // submitted count = 1 < 3 → task.status = awaiting_consensus.
        let Some((pool, _guard)) = pool_or_skip().await else {
            return;
        };

        let job_id = Uuid::new_v4();
        insert_job_row(&pool, job_id, "processing").await;
        let task_id = Uuid::new_v4();
        insert_task_row_rt(&pool, task_id, job_id, "assigned", 0, &["x"], 3).await;

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

        let outcome = submit_task(&pool, task_id, worker_id, "only", "", 1.0)
            .await
            .unwrap();
        // Not completed yet — job must still be processing.
        assert_eq!(
            outcome,
            SubmitOutcome::Submitted { job_terminal: None }
        );
        let (status, _) = get_task_status_and_attempts(&pool, task_id).await;
        assert_eq!(status, "awaiting_consensus");
        // winning_assignment_id remains NULL because consensus was skipped.
        assert!(get_winning_assignment_id(&pool, task_id).await.is_none());
        assert_eq!(get_job_status(&pool, job_id).await, "processing");
    }

    #[tokio::test]
    async fn test_submit_task_calls_consensus_at_target() {
        // redundancy_target = 2; two workers submit SAME stdout (=> same
        // result_hash). After the 2nd submit consensus must resolve to
        // Completed and set winning_assignment_id.
        let Some((pool, _guard)) = pool_or_skip().await else {
            return;
        };

        let job_id = Uuid::new_v4();
        insert_job_row(&pool, job_id, "processing").await;
        let task_id = Uuid::new_v4();
        insert_task_row_rt(&pool, task_id, job_id, "assigned", 0, &["x"], 2).await;

        let a1 = Uuid::new_v4();
        let a2 = Uuid::new_v4();
        let w1 = Uuid::new_v4();
        let w2 = Uuid::new_v4();
        let now = Utc::now();
        insert_assignment_row(
            &pool, a1, task_id, w1, now,
            now + Duration::seconds(60), "in_flight",
        )
        .await;
        insert_assignment_row(
            &pool, a2, task_id, w2, now,
            now + Duration::seconds(60), "in_flight",
        )
        .await;

        // Same stdout → same hash → consensus.Completed on 2nd submit.
        let stdout = "42\n";
        let o1 = submit_task(&pool, task_id, w1, stdout, "", 1.0).await.unwrap();
        assert_eq!(o1, SubmitOutcome::Submitted { job_terminal: None });
        // After the first submit, task is AwaitingConsensus.
        assert_eq!(
            get_task_status_and_attempts(&pool, task_id).await.0,
            "awaiting_consensus"
        );

        let o2 = submit_task(&pool, task_id, w2, stdout, "", 1.0).await.unwrap();
        // Second submit flips the task to Completed, and since it's the only
        // task in the job, the job also flips to Completed.
        assert_eq!(
            o2,
            SubmitOutcome::Submitted {
                job_terminal: Some(JobTerminal::Completed)
            }
        );
        let (status, _) = get_task_status_and_attempts(&pool, task_id).await;
        assert_eq!(status, "completed");
        let winner = get_winning_assignment_id(&pool, task_id)
            .await
            .expect("winning_assignment_id must be set on Completed");
        assert!(
            winner == a1 || winner == a2,
            "winner must be one of the two matching assignments"
        );
    }

    #[tokio::test]
    async fn test_fail_task_terminality_tightened_any_failed_flips_job_to_failed() {
        // Phase-4 tightening: even with one Completed sibling, a Failed
        // sibling in a fully-terminal job flips the job to `failed`.
        let Some((pool, _guard)) = pool_or_skip().await else {
            return;
        };

        let job_id = Uuid::new_v4();
        insert_job_row(&pool, job_id, "processing").await;

        // Seed BOTH tasks before submit_task runs, otherwise t1's submit
        // would see itself as the only task and flip the job to Completed
        // prematurely. Phase-4 sibling-terminality scans all tasks at the
        // moment of recompute.
        let t1 = Uuid::new_v4();
        insert_task_row_rt(&pool, t1, job_id, "assigned", 0, &["a"], 1).await;
        let a1 = Uuid::new_v4();
        let w1 = Uuid::new_v4();
        let now = Utc::now();
        insert_assignment_row(
            &pool, a1, t1, w1, now,
            now + Duration::seconds(60), "in_flight",
        )
        .await;

        let t2 = Uuid::new_v4();
        insert_task_row_rt(&pool, t2, job_id, "assigned", 0, &["b"], 1).await;
        let a2 = Uuid::new_v4();
        insert_assignment_row(
            &pool, a2, t2, Uuid::new_v4(), now,
            now + Duration::seconds(60), "in_flight",
        )
        .await;

        // Task 1: submit successfully (redundancy=1) → Completed.
        let o1 = submit_task(&pool, t1, w1, "ok", "", 1.0).await.unwrap();
        // Sibling t2 still Assigned, job stays Processing.
        assert_eq!(o1, SubmitOutcome::Submitted { job_terminal: None });
        assert_eq!(get_job_status(&pool, job_id).await, "processing");

        // Task 2: orphan-fail via `fail_task`. All siblings now terminal
        // (t1 Completed, t2 Failed); Phase-4 rule: any Failed + all terminal
        // → job Failed.
        let outcome = fail_task(&pool, t2).await.unwrap();
        assert_eq!(outcome, JobTerminalState::FlippedFailed);
        assert_eq!(get_job_status(&pool, job_id).await, "failed");
    }

    #[tokio::test]
    async fn test_submit_task_terminality_all_completed_flips_job() {
        // Two tasks, both closed out via submit_task (redundancy_target=1).
        // After both Completed, the job must flip to `completed`.
        let Some((pool, _guard)) = pool_or_skip().await else {
            return;
        };

        let job_id = Uuid::new_v4();
        insert_job_row(&pool, job_id, "processing").await;

        let t1 = Uuid::new_v4();
        insert_task_row_rt(&pool, t1, job_id, "assigned", 0, &["a"], 1).await;
        let w1 = Uuid::new_v4();
        let now = Utc::now();
        insert_assignment_row(
            &pool, Uuid::new_v4(), t1, w1, now,
            now + Duration::seconds(60), "in_flight",
        )
        .await;

        let t2 = Uuid::new_v4();
        insert_task_row_rt(&pool, t2, job_id, "assigned", 0, &["b"], 1).await;
        let w2 = Uuid::new_v4();
        insert_assignment_row(
            &pool, Uuid::new_v4(), t2, w2, now,
            now + Duration::seconds(60), "in_flight",
        )
        .await;

        // First submit: sibling still Assigned -> job stays processing.
        let o1 = submit_task(&pool, t1, w1, "ok1", "", 1.0).await.unwrap();
        assert_eq!(o1, SubmitOutcome::Submitted { job_terminal: None });
        assert_eq!(get_job_status(&pool, job_id).await, "processing");

        // Second submit: every sibling Completed → job flips Completed.
        let o2 = submit_task(&pool, t2, w2, "ok2", "", 1.0).await.unwrap();
        assert_eq!(
            o2,
            SubmitOutcome::Submitted {
                job_terminal: Some(JobTerminal::Completed)
            }
        );
        assert_eq!(get_job_status(&pool, job_id).await, "completed");
    }

    #[tokio::test]
    async fn test_get_task_stats_returns_awaiting_consensus_count() {
        let Some((pool, _guard)) = pool_or_skip().await else {
            return;
        };

        let job_id = Uuid::new_v4();
        insert_job_row(&pool, job_id, "processing").await;
        let me = Uuid::new_v4();
        let now = Utc::now();

        // Mix of statuses. Only the `awaiting_consensus` count is the
        // Phase-4 addition under test; we still sanity-check the other
        // fields so regressions in the stats query surface here.
        insert_task_row(&pool, Uuid::new_v4(), job_id, "pending", 0, &["p"]).await;
        insert_task_row(&pool, Uuid::new_v4(), job_id, "awaiting_consensus", 0, &["q"]).await;
        insert_task_row(&pool, Uuid::new_v4(), job_id, "awaiting_consensus", 0, &["r"]).await;
        insert_task_row(&pool, Uuid::new_v4(), job_id, "assigned", 0, &["s"]).await;
        insert_task_row(&pool, Uuid::new_v4(), job_id, "completed", 0, &["c"]).await;
        insert_task_row(&pool, Uuid::new_v4(), job_id, "failed", 0, &["f"]).await;

        let stats = get_task_stats(&pool, job_id, me).await.unwrap();
        assert_eq!(stats.awaiting_consensus, 2);
        assert_eq!(stats.pending, 1);
        assert_eq!(stats.completed_total, 1);
        // No live in_flight assignments attached here; no submissions either.
        assert_eq!(stats.in_flight, 0);
        assert_eq!(stats.completed_by_me, 0);
    }

    #[tokio::test]
    async fn test_fail_task_is_idempotent_with_no_inflight_assignment() {
        // Defensive: fail_task must not crash when there is no InFlight
        // assignment (e.g., only a TimedOut row exists, or no row at all).
        // It should still flip the task to Failed and leave other assignment
        // rows unchanged.
        let Some((pool, _guard)) = pool_or_skip().await else {
            return;
        };

        let job_id = Uuid::new_v4();
        insert_job_row(&pool, job_id, "processing").await;

        let task_id = Uuid::new_v4();
        // Task has attempts=2 but no InFlight assignment — only a TimedOut.
        insert_task_row(&pool, task_id, job_id, "assigned", 2, &["weird"]).await;
        let old = Uuid::new_v4();
        let now = Utc::now();
        insert_assignment_row(
            &pool,
            old,
            task_id,
            Uuid::new_v4(),
            now - Duration::seconds(300),
            now - Duration::seconds(240),
            "timed_out",
        )
        .await;

        let outcome = fail_task(&pool, task_id).await.unwrap();
        // Single task in job, no Completed siblings -> job flips to Failed.
        assert_eq!(outcome, JobTerminalState::FlippedFailed);

        let (status, attempts) = get_task_status_and_attempts(&pool, task_id).await;
        assert_eq!(status, "failed");
        // Attempts unchanged.
        assert_eq!(attempts, 2);
        // Existing TimedOut row untouched.
        assert_eq!(get_assignment_status(&pool, old).await, "timed_out");
    }
}
