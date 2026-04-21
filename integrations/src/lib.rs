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
    /// Job CSV has been split into tasks and is being dispatched to workers.
    Processing,
    /// All tasks reached a terminal state and at least one completed successfully.
    Completed,
    /// All tasks reached a terminal state and every one failed.
    Failed,
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

/// Lifecycle state of a single task row owned by the task-distribution module.
///
/// Tasks move `Pending -> Assigned -> AwaitingConsensus -> Completed` on the
/// happy redundant path (Phase 4); a single submission with
/// `redundancy_target = 1` collapses to
/// `Pending -> Assigned -> Completed`. Deadline-based reclamation keeps the
/// task in its current status while incrementing `attempts`; after
/// `attempts` reaches the cap (5) an expired assignment transitions the
/// task to `Failed`. Consensus over disagreeing hashes also resolves to
/// `Failed` via `result_aggregation::try_resolve_consensus`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "text", rename_all = "snake_case")]
pub enum TaskStatus {
    /// Ready to be picked up by a volunteer worker.
    Pending,
    /// Currently assigned to a worker with an active deadline.
    Assigned,
    /// At least one submission has been accepted but consensus has not yet
    /// been resolved (the `Submitted` count is still below
    /// `redundancy_target`, or consensus was escalated). Task remains
    /// eligible for dispatch to additional workers.
    AwaitingConsensus,
    /// A submission for this task has been accepted.
    Completed,
    /// Task reclaimed after exhausting the retry budget without a submission.
    Failed,
}

/// Lifecycle state of a single assignment row linking a task to a worker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "text", rename_all = "snake_case")]
pub enum AssignmentStatus {
    /// Assignment is active and the worker's deadline has not yet passed.
    InFlight,
    /// Worker submitted a result before the deadline.
    Submitted,
    /// The deadline expired without a submission; assignment was reclaimed.
    TimedOut,
}

/// Request body for `POST /jobs/{id}/start`.
///
/// The `chunk_size` field is present for forward compatibility but is
/// ignored in Phase 3: every task always contains exactly one CSV data row.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StartJobRequest {
    /// Optional number of CSV rows per task. Ignored in Phase 3.
    pub chunk_size: Option<u32>,
}

/// Response body for `POST /jobs/{id}/start`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StartJobResponse {
    /// The job whose CSV was split.
    pub job_id: Uuid,
    /// Number of tasks inserted for the job.
    pub task_count: u32,
}

/// Request body for `POST /tasks/next`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NextTaskRequest {
    /// Caller-chosen UUID identifying the browser worker instance.
    pub worker_id: Uuid,
}

/// Response body for `POST /tasks/next` when a task is dispatched.
///
/// `input_rows` is always length 1 in Phase 3 (see the task-distribution
/// spec's non-goals section).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskDispatch {
    /// Identifier of the dispatched task.
    pub task_id: Uuid,
    /// Identifier of the parent job.
    pub job_id: Uuid,
    /// Raw Python source for the task, read from `data/{job_id}/*.py`.
    pub script: String,
    /// CSV data rows for this task (length exactly 1 in Phase 3).
    pub input_rows: Vec<String>,
    /// Absolute deadline by which the worker must submit a result.
    pub deadline_at: DateTime<Utc>,
}

/// Request body for `POST /tasks/{id}/submit`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubmitTaskRequest {
    /// Must match the `worker_id` on the current in-flight assignment.
    pub worker_id: Uuid,
    /// Captured stdout from the Pyodide execution.
    pub stdout: String,
    /// Captured stderr from the Pyodide execution.
    pub stderr: String,
    /// Wall-clock duration of the worker-side execution, in milliseconds.
    pub duration_ms: f64,
}

/// Response body for `GET /tasks/stats`.
///
/// Field names are snake_case on the wire (serde default) and match the
/// TypeScript `TaskStats` interface exactly.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskStats {
    /// Tasks whose status is `Pending`.
    pub pending: i64,
    /// Tasks in `Assigned` or `AwaitingConsensus` whose current assignment
    /// is `InFlight` and not past the deadline.
    pub in_flight: i64,
    /// Tasks whose status is `AwaitingConsensus` for the job.
    pub awaiting_consensus: i64,
    /// Tasks in status `Completed` for the job.
    pub completed_total: i64,
    /// Submitted assignments for the job whose `worker_id` matches the caller.
    pub completed_by_me: i64,
}
