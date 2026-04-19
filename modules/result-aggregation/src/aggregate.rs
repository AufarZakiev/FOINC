//! Assemble the final CSV for a completed job.
//!
//! Reads every task's winning assignment for `job_id`, orders by
//! `tasks.chunk_index`, and concatenates the RAW stdouts (NOT normalized)
//! separated by a single `\n`. The raw stdout is preserved so the
//! scientist's formatting — trailing newlines, significant spacing,
//! whatever — survives the round trip exactly.
//!
//! Consensus-level normalization (`normalize_stdout`) is only used for
//! hash comparison; it is never applied to the response body.

use sqlx::PgPool;
use uuid::Uuid;

/// Error variants surfaced by [`assemble_result`]. The HTTP handler maps
/// these to status codes; the types here let the module tests assert
/// outcomes without hitting axum.
#[derive(Debug)]
pub enum AggregateError {
    /// No `jobs` row matches `job_id`.
    NotFound,
    /// Job exists but is still `uploaded` or `processing`.
    NotComplete,
    /// Job is `failed`; the download is not available.
    Failed,
    /// Database or serialization error. Treated as 500 by the handler.
    Sqlx(sqlx::Error),
}

impl From<sqlx::Error> for AggregateError {
    fn from(err: sqlx::Error) -> Self {
        AggregateError::Sqlx(err)
    }
}

/// Assemble the CSV body for a completed job.
///
/// Returns the concatenated stdout on success. Returns `NotFound`,
/// `NotComplete`, or `Failed` based on the job's current status. If a task
/// has no `winning_assignment_id` — which should never happen for a
/// `completed` job — the resulting row contributes an empty string so the
/// output still assembles rather than 500-ing on a defensive check.
pub async fn assemble_result(
    pool: &PgPool,
    job_id: Uuid,
) -> Result<String, AggregateError> {
    // 1. Look up the job; 404 vs 409 vs 422 vs happy path all branch off
    //    here so we do one query.
    let status: Option<String> = sqlx::query_scalar(
        r#"SELECT status FROM jobs WHERE job_id = $1"#,
    )
    .bind(job_id)
    .fetch_optional(pool)
    .await?;

    let status = match status {
        None => return Err(AggregateError::NotFound),
        Some(s) => s,
    };

    match status.as_str() {
        "completed" => {} // fall through
        "uploaded" | "processing" => return Err(AggregateError::NotComplete),
        "failed" => return Err(AggregateError::Failed),
        // Unknown status — treat as not-complete rather than 500. This is
        // defensive; the CHECK constraint added in 003 restricts values to
        // the four above.
        _ => return Err(AggregateError::NotComplete),
    }

    // 2. Pull each task's winning stdout in chunk_index order. A LEFT JOIN
    //    keeps the row even if winning_assignment_id is somehow NULL so
    //    we emit the same number of rows as there are tasks.
    let rows: Vec<(Option<String>,)> = sqlx::query_as(
        r#"
        SELECT a.stdout
          FROM tasks t
     LEFT JOIN assignments a
            ON a.assignment_id = t.winning_assignment_id
         WHERE t.job_id = $1
         ORDER BY t.chunk_index ASC
        "#,
    )
    .bind(job_id)
    .fetch_all(pool)
    .await?;

    let parts: Vec<String> = rows
        .into_iter()
        .map(|(stdout,)| stdout.unwrap_or_default())
        .collect();

    // Spec: join with a SINGLE `\n` between tasks. No leading or trailing
    // newline is added; the raw stdout is emitted as-is.
    Ok(parts.join("\n"))
}

#[cfg(test)]
mod tests {
    //! `assemble_result` tests. Each case seeds a job + tasks +
    //! assignments, calls `assemble_result`, and asserts on the String
    //! body or `AggregateError` variant. Tests skip cleanly when
    //! `DATABASE_URL` is unset.

    use super::*;
    use chrono::{Duration, Utc};
    use sqlx::postgres::PgPoolOptions;

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

    /// Seed a Completed task + its winning Submitted assignment carrying the
    /// given stdout. Returns the winning assignment id.
    async fn seed_completed_task(
        pool: &PgPool,
        job_id: Uuid,
        chunk_index: i32,
        stdout: &str,
    ) -> Uuid {
        let task_id = Uuid::new_v4();
        let assignment_id = Uuid::new_v4();
        let now = Utc::now();

        // Insert the task first so the FK on `tasks.winning_assignment_id`
        // has a target to point at after the assignment exists.
        sqlx::query(
            r#"
            INSERT INTO tasks
                (task_id, job_id, chunk_index, input_rows, status, attempts,
                 redundancy_target, created_at)
            VALUES
                ($1, $2, $3, ARRAY['x'], 'completed', 0, 1, now())
            "#,
        )
        .bind(task_id)
        .bind(job_id)
        .bind(chunk_index)
        .execute(pool)
        .await
        .unwrap();

        sqlx::query(
            r#"
            INSERT INTO assignments
                (assignment_id, task_id, worker_id, assigned_at, deadline_at,
                 status, stdout, stderr, duration_ms, result_hash)
            VALUES
                ($1, $2, $3, $4, $5, 'submitted', $6, '', 1.0, 'deadbeef')
            "#,
        )
        .bind(assignment_id)
        .bind(task_id)
        .bind(Uuid::new_v4())
        .bind(now - Duration::seconds(30))
        .bind(now + Duration::seconds(30))
        .bind(stdout)
        .execute(pool)
        .await
        .unwrap();

        sqlx::query(
            r#"UPDATE tasks SET winning_assignment_id = $1 WHERE task_id = $2"#,
        )
        .bind(assignment_id)
        .bind(task_id)
        .execute(pool)
        .await
        .unwrap();

        assignment_id
    }

    #[tokio::test]
    async fn test_assemble_result_concatenates_stdouts_in_chunk_order() {
        // Three completed tasks, chunk_index 0,1,2 with stdouts
        // "r0\n", "r1\n", "r2\n". Expected output is the three strings
        // joined by a single `\n` separator. Insertion order intentionally
        // does NOT match chunk order, so we also verify the ORDER BY.
        let Some((pool, _guard)) = pool_or_skip().await else {
            return;
        };

        let job_id = Uuid::new_v4();
        insert_job_row(&pool, job_id, "completed").await;
        let _ = seed_completed_task(&pool, job_id, 2, "r2\n").await;
        let _ = seed_completed_task(&pool, job_id, 0, "r0\n").await;
        let _ = seed_completed_task(&pool, job_id, 1, "r1\n").await;

        let body = assemble_result(&pool, job_id).await.unwrap();
        assert_eq!(body, "r0\n\nr1\n\nr2\n");
    }

    #[tokio::test]
    async fn test_assemble_result_returns_not_found_for_missing_job() {
        let Some((pool, _guard)) = pool_or_skip().await else {
            return;
        };

        let result = assemble_result(&pool, Uuid::new_v4()).await;
        assert!(matches!(result, Err(AggregateError::NotFound)));
    }

    #[tokio::test]
    async fn test_assemble_result_returns_not_complete_when_processing() {
        let Some((pool, _guard)) = pool_or_skip().await else {
            return;
        };

        let job_id = Uuid::new_v4();
        insert_job_row(&pool, job_id, "processing").await;

        let result = assemble_result(&pool, job_id).await;
        assert!(matches!(result, Err(AggregateError::NotComplete)));
    }

    #[tokio::test]
    async fn test_assemble_result_returns_failed_when_job_failed() {
        let Some((pool, _guard)) = pool_or_skip().await else {
            return;
        };

        let job_id = Uuid::new_v4();
        insert_job_row(&pool, job_id, "failed").await;

        let result = assemble_result(&pool, job_id).await;
        assert!(matches!(result, Err(AggregateError::Failed)));
    }
}
