use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

use foinc_integrations::{
    NextTaskRequest, StartJobRequest, StartJobResponse, SubmitTaskRequest, TaskDispatch,
};

use crate::csv_split;
use crate::db::{self, StartProcessingOutcome, SubmitOutcome};

/// Handler for `POST /jobs/{id}/start`.
pub async fn start_job_handler(
    State(pool): State<PgPool>,
    Path(id): Path<Uuid>,
    body: Option<axum::Json<StartJobRequest>>,
) -> Response {
    // `chunk_size` is accepted for forward compatibility but ignored.
    let _ = body;

    // Atomic CAS uploaded -> processing.
    match db::start_processing(&pool, id).await {
        Ok(StartProcessingOutcome::NotFound) => {
            return (
                StatusCode::NOT_FOUND,
                axum::Json(json!({ "error": "Job not found" })),
            )
                .into_response();
        }
        Ok(StartProcessingOutcome::Conflict) => {
            return (
                StatusCode::CONFLICT,
                axum::Json(json!({ "error": "Job is not in uploaded state" })),
            )
                .into_response();
        }
        Ok(StartProcessingOutcome::Started) => {}
        Err(_) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                axum::Json(json!({ "error": "Database error" })),
            )
                .into_response();
        }
    }

    // Read the CSV from disk and split.
    let csv_path = match db::find_job_csv(id).await {
        Ok(path) => path,
        Err(_) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                axum::Json(json!({ "error": "Failed to locate job CSV on disk" })),
            )
                .into_response();
        }
    };
    let rows = match csv_split::split_csv(&csv_path).await {
        Ok(rows) => rows,
        Err(_) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                axum::Json(json!({ "error": "Failed to read job CSV" })),
            )
                .into_response();
        }
    };

    // Insert tasks. Even when `rows` is empty we still commit — the job
    // has already moved to `processing` and the task count is a faithful
    // zero.
    let task_count = match db::insert_pending_tasks(&pool, id, &rows).await {
        Ok(n) => n,
        Err(_) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                axum::Json(json!({ "error": "Failed to insert tasks" })),
            )
                .into_response();
        }
    };

    let response = StartJobResponse {
        job_id: id,
        task_count,
    };
    (StatusCode::OK, axum::Json(json!(response))).into_response()
}

/// Handler for `POST /tasks/next`.
pub async fn next_task_handler(
    State(pool): State<PgPool>,
    axum::Json(req): axum::Json<NextTaskRequest>,
) -> Response {
    let picked = match db::pick_next_task(&pool, req.worker_id).await {
        Ok(Some(p)) => p,
        Ok(None) => return StatusCode::NO_CONTENT.into_response(),
        Err(_) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                axum::Json(json!({ "error": "Database error" })),
            )
                .into_response();
        }
    };

    let script = match db::read_job_script(picked.job_id).await {
        Ok(s) => s,
        Err(_) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                axum::Json(json!({ "error": "Failed to read job script" })),
            )
                .into_response();
        }
    };

    let dispatch = TaskDispatch {
        task_id: picked.task_id,
        job_id: picked.job_id,
        script,
        input_rows: picked.input_rows,
        deadline_at: picked.deadline_at,
    };
    (StatusCode::OK, axum::Json(json!(dispatch))).into_response()
}

/// Handler for `POST /tasks/{id}/submit`.
pub async fn submit_task_handler(
    State(pool): State<PgPool>,
    Path(id): Path<Uuid>,
    axum::Json(req): axum::Json<SubmitTaskRequest>,
) -> Response {
    match db::submit_task(
        &pool,
        id,
        req.worker_id,
        &req.stdout,
        &req.stderr,
        req.duration_ms,
    )
    .await
    {
        Ok(SubmitOutcome::NotFound) => (
            StatusCode::NOT_FOUND,
            axum::Json(json!({ "error": "Task or assignment not found" })),
        )
            .into_response(),
        Ok(SubmitOutcome::Conflict) => (
            StatusCode::CONFLICT,
            axum::Json(json!({ "error": "Assignment is not accepting this submission" })),
        )
            .into_response(),
        Ok(SubmitOutcome::Submitted { .. }) => {
            (StatusCode::OK, axum::Json(json!({}))).into_response()
        }
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            axum::Json(json!({ "error": "Database error" })),
        )
            .into_response(),
    }
}

/// Query parameters for `GET /tasks/stats`.
#[derive(Debug, Deserialize)]
pub struct TaskStatsQuery {
    pub job_id: Uuid,
    pub worker_id: Uuid,
}

/// Handler for `GET /tasks/stats`.
pub async fn task_stats_handler(
    State(pool): State<PgPool>,
    Query(q): Query<TaskStatsQuery>,
) -> Response {
    match db::job_exists(&pool, q.job_id).await {
        Ok(true) => {}
        Ok(false) => {
            return (
                StatusCode::NOT_FOUND,
                axum::Json(json!({ "error": "Job not found" })),
            )
                .into_response();
        }
        Err(_) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                axum::Json(json!({ "error": "Database error" })),
            )
                .into_response();
        }
    }

    match db::get_task_stats(&pool, q.job_id, q.worker_id).await {
        Ok(stats) => (StatusCode::OK, axum::Json(json!(stats))).into_response(),
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            axum::Json(json!({ "error": "Database error" })),
        )
            .into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::extract::{Path, Query, State};
    use chrono::{Duration, Utc};
    use http_body_util::BodyExt;
    use sqlx::postgres::PgPoolOptions;
    use std::path::PathBuf;

    use foinc_integrations::{NextTaskRequest, StartJobRequest, SubmitTaskRequest, TaskStats};

    use crate::test_support::{reset_db, DB_LOCK, ENV_LOCK};

    /// RAII guard that sets `DATA_DIR` to a unique temp directory on creation
    /// and cleans up on drop. Uses the crate-wide `ENV_LOCK` so that
    /// concurrent tests serialize their `DATA_DIR` mutations; otherwise
    /// `cargo test`'s default parallelism clobbers the env var.
    struct DataDirGuard {
        dir: PathBuf,
        _lock: std::sync::MutexGuard<'static, ()>,
    }

    impl DataDirGuard {
        fn new() -> Self {
            let lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            let dir = std::env::temp_dir().join(format!("foinc_test_td_{}", Uuid::new_v4()));
            std::fs::create_dir_all(&dir).unwrap();
            std::env::set_var("DATA_DIR", dir.to_str().unwrap());
            Self { dir, _lock: lock }
        }

        fn path(&self) -> &std::path::Path {
            &self.dir
        }
    }

    impl Drop for DataDirGuard {
        fn drop(&mut self) {
            std::env::remove_var("DATA_DIR");
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    /// Connect + acquire the crate-wide `DB_LOCK` and TRUNCATE the tables
    /// so tests don't see each other's fixtures. See `db.rs::pool_or_skip`
    /// for the rationale.
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

    /// Collect a response body into bytes (for JSON assertions).
    async fn body_bytes(response: axum::response::Response) -> Vec<u8> {
        response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .to_vec()
    }

    async fn body_json(response: axum::response::Response) -> serde_json::Value {
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

    /// Write a pair of `data/{job_id}/{csv,py}` files so that start_job_handler
    /// and next_task_handler can locate them.
    fn write_job_files(data_dir: &std::path::Path, job_id: Uuid, csv: &str, script: &str) {
        let dir = data_dir.join(job_id.to_string());
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("data.csv"), csv).unwrap();
        std::fs::write(dir.join("run.py"), script).unwrap();
    }

    // -------------------------------------------------------------------
    // start_job_handler
    // -------------------------------------------------------------------

    #[tokio::test]
    async fn test_start_job_handler_returns_200_and_task_count() {
        let guard = DataDirGuard::new();
        let Some((pool, _guard)) = pool_or_skip().await else {
            return;
        };

        let job_id = Uuid::new_v4();
        insert_job_row(&pool, job_id, "uploaded").await;
        write_job_files(
            guard.path(),
            job_id,
            "col1,col2\n1,2\n3,4\n5,6\n",
            "print('hi')",
        );

        let response = start_job_handler(
            State(pool.clone()),
            Path(job_id),
            Some(axum::Json(StartJobRequest { chunk_size: None })),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);

        let body = body_json(response).await;
        assert_eq!(body["job_id"], job_id.to_string());
        assert_eq!(body["task_count"], 3);

        // Side-effect: job now Processing.
        let status: String =
            sqlx::query_scalar(r#"SELECT status FROM jobs WHERE job_id = $1"#)
                .bind(job_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(status, "processing");
    }

    #[tokio::test]
    async fn test_start_job_handler_returns_404_on_missing_job() {
        let _guard = DataDirGuard::new();
        let Some((pool, _guard)) = pool_or_skip().await else {
            return;
        };

        let response = start_job_handler(
            State(pool),
            Path(Uuid::new_v4()),
            Some(axum::Json(StartJobRequest { chunk_size: None })),
        )
        .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let body = body_json(response).await;
        assert!(body["error"].as_str().unwrap().contains("not found"));
    }

    #[tokio::test]
    async fn test_start_job_handler_returns_409_when_already_processing() {
        let _guard = DataDirGuard::new();
        let Some((pool, _guard)) = pool_or_skip().await else {
            return;
        };

        let job_id = Uuid::new_v4();
        insert_job_row(&pool, job_id, "processing").await;

        let response = start_job_handler(
            State(pool),
            Path(job_id),
            Some(axum::Json(StartJobRequest { chunk_size: None })),
        )
        .await;
        assert_eq!(response.status(), StatusCode::CONFLICT);
    }

    // -------------------------------------------------------------------
    // next_task_handler
    // -------------------------------------------------------------------

    #[tokio::test]
    async fn test_next_task_handler_returns_200_with_dispatch() {
        let guard = DataDirGuard::new();
        let Some((pool, _guard)) = pool_or_skip().await else {
            return;
        };

        let job_id = Uuid::new_v4();
        insert_job_row(&pool, job_id, "processing").await;
        write_job_files(
            guard.path(),
            job_id,
            "col1\n1\n",
            "print('hi from script')",
        );
        let task_id = Uuid::new_v4();
        insert_task_row(&pool, task_id, job_id, "pending", 0, &["42"]).await;

        let worker_id = Uuid::new_v4();
        let response = next_task_handler(
            State(pool),
            axum::Json(NextTaskRequest { worker_id }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);

        let body = body_json(response).await;
        assert_eq!(body["task_id"], task_id.to_string());
        assert_eq!(body["job_id"], job_id.to_string());
        assert_eq!(body["script"], "print('hi from script')");
        assert_eq!(body["input_rows"], serde_json::json!(["42"]));
        assert!(body["deadline_at"].is_string());
    }

    #[tokio::test]
    async fn test_next_task_handler_returns_204_when_queue_empty() {
        let _guard = DataDirGuard::new();
        let Some((pool, _guard)) = pool_or_skip().await else {
            return;
        };

        let response = next_task_handler(
            State(pool),
            axum::Json(NextTaskRequest {
                worker_id: Uuid::new_v4(),
            }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
    }

    // -------------------------------------------------------------------
    // submit_task_handler
    // -------------------------------------------------------------------

    #[tokio::test]
    async fn test_submit_task_handler_returns_200_on_success() {
        let _guard = DataDirGuard::new();
        let Some((pool, _guard)) = pool_or_skip().await else {
            return;
        };

        let job_id = Uuid::new_v4();
        insert_job_row(&pool, job_id, "processing").await;
        let task_id = Uuid::new_v4();
        insert_task_row(&pool, task_id, job_id, "assigned", 0, &["a"]).await;
        let worker_id = Uuid::new_v4();
        let now = Utc::now();
        insert_assignment_row(
            &pool,
            Uuid::new_v4(),
            task_id,
            worker_id,
            now,
            now + Duration::seconds(60),
            "in_flight",
        )
        .await;

        let response = submit_task_handler(
            State(pool),
            Path(task_id),
            axum::Json(SubmitTaskRequest {
                worker_id,
                stdout: "ok".into(),
                stderr: "".into(),
                duration_ms: 1.0,
            }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_json(response).await;
        assert_eq!(body, serde_json::json!({}));
    }

    #[tokio::test]
    async fn test_submit_task_handler_returns_404_when_task_missing() {
        let _guard = DataDirGuard::new();
        let Some((pool, _guard)) = pool_or_skip().await else {
            return;
        };

        let response = submit_task_handler(
            State(pool),
            Path(Uuid::new_v4()),
            axum::Json(SubmitTaskRequest {
                worker_id: Uuid::new_v4(),
                stdout: String::new(),
                stderr: String::new(),
                duration_ms: 0.0,
            }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_submit_task_handler_returns_409_on_worker_mismatch() {
        let _guard = DataDirGuard::new();
        let Some((pool, _guard)) = pool_or_skip().await else {
            return;
        };

        let job_id = Uuid::new_v4();
        insert_job_row(&pool, job_id, "processing").await;
        let task_id = Uuid::new_v4();
        insert_task_row(&pool, task_id, job_id, "assigned", 0, &["a"]).await;
        let real_worker = Uuid::new_v4();
        let now = Utc::now();
        insert_assignment_row(
            &pool,
            Uuid::new_v4(),
            task_id,
            real_worker,
            now,
            now + Duration::seconds(60),
            "in_flight",
        )
        .await;

        let response = submit_task_handler(
            State(pool),
            Path(task_id),
            axum::Json(SubmitTaskRequest {
                worker_id: Uuid::new_v4(), // wrong worker
                stdout: "".into(),
                stderr: "".into(),
                duration_ms: 0.0,
            }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::CONFLICT);
    }

    // -------------------------------------------------------------------
    // task_stats_handler
    // -------------------------------------------------------------------

    #[tokio::test]
    async fn test_task_stats_handler_returns_correct_counts() {
        let _guard = DataDirGuard::new();
        let Some((pool, _guard)) = pool_or_skip().await else {
            return;
        };

        let job_id = Uuid::new_v4();
        insert_job_row(&pool, job_id, "processing").await;
        let me = Uuid::new_v4();
        let now = Utc::now();

        // 1 pending + 1 live in_flight + 1 completed by me.
        insert_task_row(&pool, Uuid::new_v4(), job_id, "pending", 0, &["p"]).await;

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

        let done_task = Uuid::new_v4();
        insert_task_row(&pool, done_task, job_id, "completed", 1, &["c"]).await;
        insert_assignment_row(
            &pool,
            Uuid::new_v4(),
            done_task,
            me,
            now - Duration::seconds(30),
            now + Duration::seconds(30),
            "submitted",
        )
        .await;

        let response = task_stats_handler(
            State(pool),
            Query(TaskStatsQuery {
                job_id,
                worker_id: me,
            }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);

        let stats: TaskStats = serde_json::from_slice(&body_bytes(response).await).unwrap();
        assert_eq!(stats.pending, 1);
        assert_eq!(stats.in_flight, 1);
        assert_eq!(stats.completed_total, 1);
        assert_eq!(stats.completed_by_me, 1);
    }

    #[tokio::test]
    async fn test_task_stats_handler_returns_404_on_missing_job() {
        let _guard = DataDirGuard::new();
        let Some((pool, _guard)) = pool_or_skip().await else {
            return;
        };

        let response = task_stats_handler(
            State(pool),
            Query(TaskStatsQuery {
                job_id: Uuid::new_v4(),
                worker_id: Uuid::new_v4(),
            }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
}
