/**
 * TypeScript mirror of shared Rust types from `integrations/src/`.
 *
 * Keep field names and types in sync with the Rust definitions; these are
 * the wire shapes crossing the HTTP boundary.
 */

/** Status of a job as it moves through the processing pipeline. */
export type JobStatus = "uploaded" | "processing" | "completed" | "failed";

/** Metadata for a submitted job. Matches the Rust `Job` struct. */
export interface Job {
  job_id: string;
  csv_filename: string;
  script_filename: string;
  csv_size_bytes: number;
  script_size_bytes: number;
  status: JobStatus;
  created_at: string;
}

/**
 * Payload returned by `POST /tasks/next` when a task is dispatched.
 *
 * Mirrors the Rust `TaskDispatch` struct. `input_rows` always has length 1
 * in Phase 3; consumers should read `input_rows[0]`.
 */
export interface TaskDispatch {
  task_id: string;
  job_id: string;
  script: string;
  input_rows: string[];
  deadline_at: string;
}

/**
 * Progress statistics for a job as seen by the calling worker.
 *
 * Mirrors the Rust `TaskStats` struct. Field names are snake_case on the
 * wire (no serde rename); keep this shape aligned with the Rust definition.
 */
export interface TaskStats {
  pending: number;
  in_flight: number;
  completed_total: number;
  completed_by_me: number;
}
