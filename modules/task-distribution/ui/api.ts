import type { TaskDispatch, TaskStats } from "../../../integrations/ui/types";

/**
 * Response from `POST /api/jobs/{id}/start`. Mirrors the Rust
 * `StartJobResponse` struct.
 */
export interface StartJobResponse {
  job_id: string;
  task_count: number;
}

/**
 * Request body for `POST /api/tasks/{id}/submit`. Mirrors the Rust
 * `SubmitTaskRequest` struct.
 */
export interface SubmitTaskRequest {
  worker_id: string;
  stdout: string;
  stderr: string;
  duration_ms: number;
}

/**
 * Start a previously-uploaded job. Proxies to backend `POST /jobs/{id}/start`.
 *
 * `chunkSize` is accepted for forward compatibility but is ignored by the
 * backend in Phase 3 (one CSV row per task).
 *
 * Throws on non-2xx with the message extracted from the response body.
 */
export async function startJob(
  jobId: string,
  chunkSize?: number,
): Promise<StartJobResponse> {
  const response = await fetch(`/api/jobs/${jobId}/start`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ chunk_size: chunkSize ?? null }),
  });

  if (!response.ok) {
    const body = await response
      .json()
      .catch(() => ({ error: "Start job failed" }));
    throw new Error(body.error ?? `Start job failed with status ${response.status}`);
  }

  return response.json();
}

/**
 * Ask the backend for the next task available to this worker. Proxies to
 * `POST /api/tasks/next`.
 *
 * Resolves `null` on `204 No Content` (nothing to do right now).
 * Resolves a `TaskDispatch` on `200`. Throws on any other non-2xx status.
 */
export async function pollNextTask(
  workerId: string,
): Promise<TaskDispatch | null> {
  const response = await fetch("/api/tasks/next", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ worker_id: workerId }),
  });

  if (response.status === 204) {
    return null;
  }
  if (!response.ok) {
    const body = await response
      .json()
      .catch(() => ({ error: "Poll failed" }));
    throw new Error(body.error ?? `Poll failed with status ${response.status}`);
  }

  return response.json();
}

/**
 * Submit a completed task. Proxies to `POST /api/tasks/{taskId}/submit`.
 *
 * Resolves on `200`. Throws on non-2xx.
 */
export async function submitTask(
  taskId: string,
  req: SubmitTaskRequest,
): Promise<void> {
  const response = await fetch(`/api/tasks/${taskId}/submit`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(req),
  });

  if (!response.ok) {
    const body = await response
      .json()
      .catch(() => ({ error: "Submit failed" }));
    throw new Error(body.error ?? `Submit failed with status ${response.status}`);
  }
}

/**
 * Read current dispatch statistics for `(jobId, workerId)`. Proxies to
 * `GET /api/tasks/stats?job_id=...&worker_id=...`.
 *
 * Throws on non-2xx with the message extracted from the response body.
 */
export async function getTaskStats(
  jobId: string,
  workerId: string,
): Promise<TaskStats> {
  const params = new URLSearchParams({
    job_id: jobId,
    worker_id: workerId,
  });
  const response = await fetch(`/api/tasks/stats?${params.toString()}`);

  if (!response.ok) {
    const body = await response
      .json()
      .catch(() => ({ error: "Stats fetch failed" }));
    throw new Error(body.error ?? `Stats fetch failed with status ${response.status}`);
  }

  return response.json();
}
