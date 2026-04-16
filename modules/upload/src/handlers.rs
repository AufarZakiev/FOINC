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
