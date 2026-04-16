use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Status of a job as it moves through the processing pipeline.
///
/// The `Uploaded` variant is set by the upload module. Downstream modules
/// (preview, task-distribution, etc.) add further variants as the job
/// progresses.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "text", rename_all = "snake_case")]
pub enum JobStatus {
    /// Job files have been uploaded and metadata persisted.
    Uploaded,
}

/// Metadata for a submitted job.
///
/// Created by the upload module when a scientist uploads a CSV data file
/// and a Python script. Downstream modules read and update this record as
/// the job moves through the pipeline.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Job {
    /// Unique identifier for the job (UUID v4).
    pub job_id: Uuid,
    /// Original filename of the uploaded CSV file.
    pub csv_filename: String,
    /// Original filename of the uploaded Python script.
    pub script_filename: String,
    /// Size of the CSV file in bytes.
    pub csv_size_bytes: i64,
    /// Size of the Python script in bytes.
    pub script_size_bytes: i64,
    /// Current status of the job.
    pub status: JobStatus,
    /// Timestamp when the job was created.
    pub created_at: DateTime<Utc>,
}

/// Response type for the `POST /upload` endpoint.
pub type UploadResponse = Job;
