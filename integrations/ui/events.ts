/**
 * Cross-module UI contracts.
 *
 * Types in this file are the only way two frontend modules may communicate.
 * A module's spec may reference a type defined here; a module's spec may NOT
 * reference another module's spec or component directly.
 */

/**
 * Payload emitted by the upload module once files have been accepted by the
 * backend. Consumed by downstream modules (e.g. pyodide-runtime) that need
 * access to the uploaded script and CSV without forcing the user to
 * re-enter them.
 */
export interface UploadCompleted {
  /** UUID v4 assigned by the backend, matches `Job.job_id`. */
  jobId: string;
  /** Raw Python source as uploaded (not a path). */
  script: string;
  /** Raw CSV text as uploaded (not a path). Includes header line. */
  csv: string;
}
