use axum::extract::{Multipart, Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use chrono::Utc;
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

use foinc_integrations::{Job, JobStatus};

use crate::db;
use crate::storage;
use crate::validation::{self, UploadFile};

/// Handler for `POST /upload`.
///
/// Accepts a multipart form with `csv_file` and `script_file` fields.
/// Validates files, writes them to disk, inserts job metadata into Postgres,
/// and returns the job as JSON with status 201.
pub async fn upload_handler(State(pool): State<PgPool>, mut multipart: Multipart) -> Response {
    let mut csv_file: Option<UploadFile> = None;
    let mut script_file: Option<UploadFile> = None;

    // Extract fields from multipart
    while let Ok(Some(field)) = multipart.next_field().await {
        let field_name = match field.name() {
            Some(name) => name.to_string(),
            None => continue,
        };
        let filename = match field.file_name() {
            Some(name) => name.to_string(),
            None => continue,
        };
        let data = match field.bytes().await {
            Ok(bytes) => bytes.to_vec(),
            Err(_) => {
                return (
                    StatusCode::BAD_REQUEST,
                    axum::Json(json!({ "error": "Failed to read file data" })),
                )
                    .into_response();
            }
        };

        match field_name.as_str() {
            "csv_file" => {
                csv_file = Some(UploadFile { filename, data });
            }
            "script_file" => {
                script_file = Some(UploadFile { filename, data });
            }
            _ => {}
        }
    }

    // Check required fields
    let csv = match csv_file {
        Some(f) => f,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                axum::Json(json!({ "error": "Missing required field: csv_file" })),
            )
                .into_response();
        }
    };
    let script = match script_file {
        Some(f) => f,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                axum::Json(json!({ "error": "Missing required field: script_file" })),
            )
                .into_response();
        }
    };

    // Validate
    if let Err(e) = validation::validate_upload(&csv, &script) {
        return e.into_response();
    }

    // Generate job metadata
    let job_id = Uuid::new_v4();
    let job = Job {
        job_id,
        csv_filename: csv.filename.clone(),
        script_filename: script.filename.clone(),
        csv_size_bytes: csv.data.len() as i64,
        script_size_bytes: script.data.len() as i64,
        status: JobStatus::Uploaded,
        created_at: Utc::now(),
    };

    // Store files on disk
    if let Err(_) = storage::store_files(job_id, csv, script).await {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            axum::Json(json!({ "error": "Failed to write files to disk" })),
        )
            .into_response();
    }

    // Insert into database
    if let Err(_) = db::insert_job(&pool, &job).await {
        // Clean up files on DB failure
        let _ = storage::cleanup_files(job_id).await;
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            axum::Json(json!({ "error": "Failed to insert job into database" })),
        )
            .into_response();
    }

    (StatusCode::CREATED, axum::Json(json!(job))).into_response()
}

/// Handler for `GET /jobs/{id}`.
///
/// Returns job metadata as JSON, or 404 if the job does not exist.
pub async fn get_job_handler(
    State(pool): State<PgPool>,
    Path(id): Path<Uuid>,
) -> Response {
    match db::get_job(&pool, id).await {
        Ok(Some(job)) => (StatusCode::OK, axum::Json(json!(job))).into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            axum::Json(json!({ "error": "Job not found" })),
        )
            .into_response(),
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            axum::Json(json!({ "error": "Database error" })),
        )
            .into_response(),
    }
}

/// Handler for `DELETE /jobs/{id}`.
///
/// Verifies the job exists, deletes the `data/{job_id}/` directory on disk,
/// then deletes the row from the `jobs` table. Returns `204 No Content` on
/// success. Returns `404` if the job does not exist (without touching disk).
/// Returns `500` on IO or DB failure; on IO failure the DB row is NOT touched.
pub async fn delete_job_handler(
    State(pool): State<PgPool>,
    Path(id): Path<Uuid>,
) -> Response {
    // 1. Check existence first — per spec, a missing row returns 404 without
    //    touching disk.
    match db::get_job(&pool, id).await {
        Ok(Some(_)) => {}
        Ok(None) => {
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

    // 2. Delete files first. If this fails, do NOT touch the DB.
    if let Err(e) = storage::cleanup_files(id).await {
        eprintln!("delete_job: failed to remove data/{}/: {}", id, e);
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            axum::Json(json!({ "error": "Failed to delete job files" })),
        )
            .into_response();
    }

    // 3. Delete the DB row. If this fails after files have been removed,
    //    log and return 500 — the record will 404 on subsequent GETs and the
    //    filesystem is already gone.
    match db::delete_job(&pool, id).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => {
            // Row vanished between the existence check and the delete — the
            // filesystem side is already gone, so from the caller's point of
            // view the job no longer exists. Report as 500 because the
            // invariant (files existed, row existed) did not hold.
            eprintln!(
                "delete_job: row for {} disappeared between check and delete",
                id
            );
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                axum::Json(json!({ "error": "Database inconsistency during delete" })),
            )
                .into_response()
        }
        Err(e) => {
            eprintln!(
                "delete_job: files for {} removed but DB delete failed: {}",
                id, e
            );
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                axum::Json(json!({ "error": "Failed to delete job from database" })),
            )
                .into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::extract::{Path, State};
    use sqlx::postgres::PgPoolOptions;
    use std::path::PathBuf;

    use crate::test_support::ENV_LOCK;
    use crate::validation::UploadFile;

    /// RAII guard that sets DATA_DIR to a unique temp directory on creation
    /// and cleans up on drop. Uses the crate-wide `ENV_LOCK` so that storage
    /// and handler tests serialize their `DATA_DIR` mutations against one
    /// another (otherwise `cargo test` parallelism clobbers the env var).
    struct DataDirGuard {
        dir: PathBuf,
        _lock: std::sync::MutexGuard<'static, ()>,
    }

    impl DataDirGuard {
        fn new() -> Self {
            // Recover from a poisoned lock — a previous test panic poisons
            // the Mutex, but the data it protects (nothing, just a guard
            // marker) is still safe to use.
            let lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            let dir = std::env::temp_dir().join(format!("foinc_test_{}", Uuid::new_v4()));
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

    /// Connect to the test Postgres instance configured via `DATABASE_URL`.
    ///
    /// Returns `None` when the env var is unset so local runs without the DB
    /// stack skip these integration tests.
    async fn pool_or_skip() -> Option<sqlx::PgPool> {
        let url = std::env::var("DATABASE_URL").ok()?;
        let pool = PgPoolOptions::new()
            .max_connections(2)
            .connect(&url)
            .await
            .expect("DATABASE_URL set but connection failed");
        Some(pool)
    }

    async fn insert_fixture_job(pool: &sqlx::PgPool, job_id: Uuid) {
        let job = Job {
            job_id,
            csv_filename: "data.csv".to_string(),
            script_filename: "run.py".to_string(),
            csv_size_bytes: 3,
            script_size_bytes: 4,
            status: JobStatus::Uploaded,
            created_at: Utc::now(),
        };
        db::insert_job(pool, &job).await.unwrap();
    }

    async fn write_fixture_files(job_id: Uuid) {
        let csv = UploadFile {
            filename: "data.csv".to_string(),
            data: b"abc".to_vec(),
        };
        let script = UploadFile {
            filename: "run.py".to_string(),
            data: b"pass".to_vec(),
        };
        crate::storage::store_files(job_id, csv, script)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_delete_handler_returns_204_and_removes_row_and_directory() {
        let guard = DataDirGuard::new();
        let Some(pool) = pool_or_skip().await else {
            return;
        };

        let job_id = Uuid::new_v4();
        insert_fixture_job(&pool, job_id).await;
        write_fixture_files(job_id).await;

        let job_dir = guard.path().join(job_id.to_string());
        assert!(job_dir.is_dir(), "precondition: job directory exists");

        let response = delete_job_handler(State(pool.clone()), Path(job_id)).await;
        assert_eq!(response.status(), StatusCode::NO_CONTENT);

        // Row is gone.
        let after = db::get_job(&pool, job_id).await.unwrap();
        assert!(after.is_none(), "DB row should be removed");

        // Directory is gone.
        assert!(
            !job_dir.exists(),
            "data/{{job_id}}/ directory should be removed"
        );
    }

    #[tokio::test]
    async fn test_delete_handler_returns_404_when_job_missing() {
        let _guard = DataDirGuard::new();
        let Some(pool) = pool_or_skip().await else {
            return;
        };

        let job_id = Uuid::new_v4();
        // No insert, no directory — clean miss.

        let response = delete_job_handler(State(pool), Path(job_id)).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_delete_handler_does_not_touch_disk_when_row_missing() {
        let guard = DataDirGuard::new();
        let Some(pool) = pool_or_skip().await else {
            return;
        };

        let job_id = Uuid::new_v4();
        // Create a directory named like the UUID but insert NO DB row. If the
        // handler touches disk before checking the DB row, this directory
        // would be removed — spec says it must not.
        let fake_dir = guard.path().join(job_id.to_string());
        std::fs::create_dir_all(&fake_dir).unwrap();
        std::fs::write(fake_dir.join("marker.txt"), b"keep me").unwrap();
        assert!(fake_dir.exists());

        let response = delete_job_handler(State(pool), Path(job_id)).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);

        // Directory must still be there — handler must not have touched disk.
        assert!(
            fake_dir.exists(),
            "handler must NOT remove directory when DB row is missing"
        );
        assert!(fake_dir.join("marker.txt").exists());
    }

    // Note: testing the 500-on-filesystem-failure path would require
    // mocking the filesystem (making `cleanup_files` fail after the
    // existence check succeeds). We have no IO mocking layer here and the
    // spec explicitly notes this branch is hard to cover; skip per the
    // test plan.
}
