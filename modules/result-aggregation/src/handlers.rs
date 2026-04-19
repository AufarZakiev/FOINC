//! HTTP handler for `GET /jobs/{id}/result`.

use axum::extract::{Path, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

use crate::aggregate::{assemble_result, AggregateError};

/// Handler for `GET /jobs/{id}/result`.
///
/// Streams the assembled CSV back with:
/// - `Content-Type: text/csv; charset=utf-8`
/// - `Content-Disposition: attachment; filename="job-<short-id>.csv"`
///   where `<short-id>` is the first 8 hex characters of the UUID (no
///   hyphen).
///
/// Status codes follow the spec:
/// - `200` body = raw CSV
/// - `404` when no job row matches
/// - `409` when the job is `uploaded` or `processing`
/// - `422` when the job is `failed`
/// - `500` on any DB error
///
/// The body is recomputed on every request — no caching, no ETag — so
/// that a future job that re-resolves consensus can serve the latest
/// result without additional invalidation work.
pub async fn get_result_handler(
    State(pool): State<PgPool>,
    Path(id): Path<Uuid>,
) -> Response {
    match assemble_result(&pool, id).await {
        Ok(body) => {
            let short_id: String = id.simple().to_string().chars().take(8).collect();
            let disposition = format!(r#"attachment; filename="job-{}.csv""#, short_id);
            (
                StatusCode::OK,
                [
                    (header::CONTENT_TYPE, "text/csv; charset=utf-8".to_string()),
                    (header::CONTENT_DISPOSITION, disposition),
                ],
                body,
            )
                .into_response()
        }
        Err(AggregateError::NotFound) => (
            StatusCode::NOT_FOUND,
            axum::Json(json!({ "error": "Job not found" })),
        )
            .into_response(),
        Err(AggregateError::NotComplete) => (
            StatusCode::CONFLICT,
            axum::Json(json!({ "error": "Job not yet complete" })),
        )
            .into_response(),
        Err(AggregateError::Failed) => (
            StatusCode::UNPROCESSABLE_ENTITY,
            axum::Json(json!({ "error": "Job failed" })),
        )
            .into_response(),
        Err(AggregateError::Sqlx(_)) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            axum::Json(json!({ "error": "Database error" })),
        )
            .into_response(),
    }
}

#[cfg(test)]
mod tests {
    //! Handler-level tests for `GET /jobs/{id}/result`. These invoke
    //! [`get_result_handler`] directly (axum-handler-test style) and
    //! assert on the returned `Response` — status code, headers, and
    //! body — without standing up the full router. Tests skip cleanly
    //! when `DATABASE_URL` is unset.
    //!
    //! Each test seeds its own jobs/tasks/assignments, serialized via
    //! the crate-local `DB_LOCK` (see `crate::test_support`).
    use super::*;
    use chrono::{Duration, Utc};
    use http_body_util::BodyExt;
    use sqlx::postgres::PgPoolOptions;

    use crate::test_support::{reset_db, DB_LOCK};

    /// Acquire a pool + the crate-wide `DB_LOCK` and TRUNCATE the shared
    /// tables. Returns `None` when `DATABASE_URL` is unset so the test
    /// exits quietly on machines without a Postgres fixture.
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

    /// Collect a response body into raw bytes.
    async fn body_bytes(response: Response) -> Vec<u8> {
        response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .to_vec()
    }

    async fn body_string(response: Response) -> String {
        String::from_utf8(body_bytes(response).await).unwrap()
    }

    async fn body_json(response: Response) -> serde_json::Value {
        let bytes = body_bytes(response).await;
        serde_json::from_slice(&bytes).unwrap()
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

    /// Seed a Completed task + its winning Submitted assignment carrying
    /// the given stdout. Mirrors the helper in `aggregate::tests`.
    async fn seed_completed_task(
        pool: &PgPool,
        job_id: Uuid,
        chunk_index: i32,
        stdout: &str,
    ) {
        let task_id = Uuid::new_v4();
        let assignment_id = Uuid::new_v4();
        let now = Utc::now();

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
    }

    #[tokio::test]
    async fn test_get_result_handler_returns_200_text_csv_for_completed_job() {
        // Happy path: a completed job with three tasks, each with a winning
        // submitted assignment carrying stdout. Handler should return 200,
        // Content-Type `text/csv; charset=utf-8`, a Content-Disposition with
        // `filename="job-<8hex>.csv"`, and a body matching the raw stdouts
        // joined by `\n`.
        let Some((pool, _guard)) = pool_or_skip().await else {
            return;
        };

        let job_id = Uuid::new_v4();
        insert_job_row(&pool, job_id, "completed").await;
        seed_completed_task(&pool, job_id, 0, "alpha").await;
        seed_completed_task(&pool, job_id, 1, "beta").await;
        seed_completed_task(&pool, job_id, 2, "gamma").await;

        let response = get_result_handler(State(pool), Path(job_id)).await;
        assert_eq!(response.status(), StatusCode::OK);

        let ct = response
            .headers()
            .get(header::CONTENT_TYPE)
            .expect("Content-Type header present")
            .to_str()
            .unwrap()
            .to_string();
        assert_eq!(ct, "text/csv; charset=utf-8");

        let cd = response
            .headers()
            .get(header::CONTENT_DISPOSITION)
            .expect("Content-Disposition header present")
            .to_str()
            .unwrap()
            .to_string();
        let short_id: String = job_id.simple().to_string().chars().take(8).collect();
        assert!(
            cd.contains(&format!(r#"filename="job-{}.csv""#, short_id)),
            "unexpected Content-Disposition: {}",
            cd
        );

        let body = body_string(response).await;
        assert_eq!(body, "alpha\nbeta\ngamma");
    }

    #[tokio::test]
    async fn test_get_result_handler_returns_404_for_missing_job() {
        // Nonexistent job_id → 404 with `{"error":"Job not found"}`.
        let Some((pool, _guard)) = pool_or_skip().await else {
            return;
        };

        let response = get_result_handler(State(pool), Path(Uuid::new_v4())).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);

        let body = body_json(response).await;
        assert_eq!(body, serde_json::json!({ "error": "Job not found" }));
    }

    #[tokio::test]
    async fn test_get_result_handler_returns_409_for_processing_job() {
        // Job still processing → 409 with `{"error":"Job not yet complete"}`.
        let Some((pool, _guard)) = pool_or_skip().await else {
            return;
        };

        let job_id = Uuid::new_v4();
        insert_job_row(&pool, job_id, "processing").await;

        let response = get_result_handler(State(pool), Path(job_id)).await;
        assert_eq!(response.status(), StatusCode::CONFLICT);

        let body = body_json(response).await;
        assert_eq!(body, serde_json::json!({ "error": "Job not yet complete" }));
    }

    #[tokio::test]
    async fn test_get_result_handler_returns_422_for_failed_job() {
        // Failed job → 422 with `{"error":"Job failed"}`.
        let Some((pool, _guard)) = pool_or_skip().await else {
            return;
        };

        let job_id = Uuid::new_v4();
        insert_job_row(&pool, job_id, "failed").await;

        let response = get_result_handler(State(pool), Path(job_id)).await;
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);

        let body = body_json(response).await;
        assert_eq!(body, serde_json::json!({ "error": "Job failed" }));
    }
}
