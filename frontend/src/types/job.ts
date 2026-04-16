/** Status of a job as it moves through the processing pipeline. */
export type JobStatus = "uploaded";

/** Metadata for a submitted job. Matches the Rust `Job` struct in integrations/src/. */
export interface Job {
  job_id: string;
  csv_filename: string;
  script_filename: string;
  csv_size_bytes: number;
  script_size_bytes: number;
  status: JobStatus;
  created_at: string;
}
