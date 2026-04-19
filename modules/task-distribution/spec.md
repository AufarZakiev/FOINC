# Module: Task Distribution

## Purpose
Split an uploaded job's CSV into per-row tasks, dispatch them to browser-based volunteer workers with deadline-based reclamation, collect results, and surface live progress to volunteers.

## Database schema (module-internal)

`tasks(task_id, job_id, chunk_index, input_rows, status, attempts, created_at)` and `assignments(assignment_id, task_id, worker_id, assigned_at, deadline_at, status, stdout, stderr, duration_ms)`. Both tables are owned by this module; no other module reads or writes them. `input_rows` is a text array of length exactly 1 in Phase 3 (see non-goals).

## State Machine

### Entity: Task

| State | Event | → State | Side effect |
|-------|-------|---------|-------------|
| *(none)* | `POST /jobs/{id}/start` received | `Pending` | Insert row with `attempts = 0`. |
| `Pending` | Dispatched via `POST /tasks/next` | `Assigned` | Insert `Assignment` (`InFlight`, `deadline_at = now() + 60s`). `attempts` is NOT incremented on initial dispatch. |
| `Assigned` | Matching `POST /tasks/{id}/submit` received in time | `Completed` | Mark assignment `Submitted`; store stdout/stderr/duration_ms. Recompute parent Job terminality (see `/submit` side effects). |
| `Assigned` | `POST /tasks/next` sees `deadline_at < now()` AND `attempts < 5` | `Pending` | Increment `attempts`, mark old assignment `TimedOut`. Eligible for redispatch in the same transaction. |
| `Assigned` | `POST /tasks/next` sees `deadline_at < now()` AND `attempts >= 5` | `Failed` | Check is evaluated BEFORE incrementing: if already at 5, mark assignment `TimedOut`, mark task `Failed`, do NOT redispatch. Handler loops for another candidate. |

Reclamation is lazy: only the next `POST /tasks/next` notices a missed deadline.

### Entity: Assignment

| State | Event | → State | Side effect |
|-------|-------|---------|-------------|
| *(none)* | Task dispatched via `POST /tasks/next` | `InFlight` | Row created with `assigned_at = now()`, `deadline_at = now() + 60s`. |
| `InFlight` | Matching `POST /tasks/{id}/submit` received, deadline not passed | `Submitted` | Persist stdout/stderr/duration_ms. |
| `InFlight` | Picker observes `deadline_at < now()` on reclamation path | `TimedOut` | Parent task transitions per table above. |

Unlisted transitions are invalid.

## API / Interface

### Error response schema

All error responses use `{ "error": "string" }`.

### Shared types (defined in `integrations/src/`)

| Type | Definition |
|------|------------|
| `JobStatus` | Extend enum with `Processing`, `Completed`, `Failed` (in addition to `Uploaded`). Rust uses `#[sqlx(type_name = "text", rename_all = "snake_case")]`; serde default serializes as snake_case. Wire values: `"uploaded"`, `"processing"`, `"completed"`, `"failed"`. |
| `TaskStatus` | Enum: `Pending`, `Assigned`, `Completed`, `Failed`. |
| `AssignmentStatus` | Enum: `InFlight`, `Submitted`, `TimedOut`. |
| `StartJobRequest` | `chunk_size: Option<u32>` (ignored in Phase 3 — see non-goals). |
| `StartJobResponse` | `job_id: Uuid`, `task_count: u32`. |
| `NextTaskRequest` | `worker_id: Uuid`. |
| `TaskDispatch` | `task_id: Uuid`, `job_id: Uuid`, `script: String`, `input_rows: Vec<String>` (length 1), `deadline_at: DateTime<Utc>`. |
| `SubmitTaskRequest` | `worker_id: Uuid`, `stdout: String`, `stderr: String`, `duration_ms: f64`. |
| `TaskStats` | `pending: i64`, `in_flight: i64`, `completed_total: i64`, `completed_by_me: i64`. |

`Task` and `Assignment` are module-internal DB shapes and are NOT in `integrations/`.

### Shared UI types (defined in `integrations/ui/`)

| File | Addition |
|------|----------|
| `integrations/ui/types.ts` | Extend `JobStatus` union to `"uploaded" \| "processing" \| "completed" \| "failed"`. Add `TaskDispatch` and `TaskStats` interfaces mirroring the Rust shapes. `TaskStats` TS shape: `{ pending: number; in_flight: number; completed_total: number; completed_by_me: number }` — field names match Rust snake_case (serde default), no renaming on the wire. |
| `integrations/ui/events.ts` | Add `JobStarted { jobId: string; taskCount: number }` with a JSDoc block matching the `UploadCompleted` pattern: emitter is `StartJobButton` on `POST /jobs/{id}/start` success; consumer is the shell which advances the wizard to the volunteer view; payload carries the backend-assigned `jobId` and number of tasks inserted. |

### `POST /jobs/{id}/start`

Split a previously uploaded job's CSV into tasks and move the job to `Processing`.

**Path:** `id: UUID`, must reference a job in state `uploaded`.

**Request body** matches `StartJobRequest` (`chunk_size` ignored).

**Response `200 OK`** matches `StartJobResponse`.

**Errors:** `404` (no such job), `409` (job not in `uploaded`), `500` (CSV read / DB failure).

**Side effects**

1. Atomic check-and-set: `UPDATE jobs SET status='processing' WHERE id=? AND status='uploaded' RETURNING ...`. Only the row that actually returned proceeds; a concurrent loser returns `409`.
2. The winning handler reads `data/{job_id}/*.csv`, splits on `\n`, trims lines, drops empties, drops the first remaining line (header).
3. Inserts one `tasks` row per remaining data line with `status = Pending`, `attempts = 0`, `input_rows = [line]` (length 1 always; any `chunk_size != 1` is ignored).
4. Returns `{ job_id, task_count }`.

---

### `POST /tasks/next`

Atomically pick the next available task for the calling worker.

**Request body** matches `NextTaskRequest`. **Response `200 OK`** matches `TaskDispatch`; **`204 No Content`** if nothing eligible. **Errors:** `500` (DB failure).

**Side effects (single DB transaction)**

1. Candidate query uses `SELECT ... FOR UPDATE SKIP LOCKED` on the `tasks` row (not the query result) so concurrent pickers never contend on the same task. Candidates are tasks where `status = Pending`, OR `status = Assigned` with the current `InFlight` assignment's `deadline_at < now()` (joined via a LATERAL / correlated subquery over `assignments`). Acquiring the task-row lock serializes the "mark old assignment `TimedOut` + create new assignment" sequence.
2. If the candidate was a timed-out `Assigned`: evaluate `attempts >= 5` FIRST. If true, mark old assignment `TimedOut`, mark task `Failed`, commit, and loop back to step 1 for another candidate. Otherwise increment `attempts`, mark old assignment `TimedOut`, and continue.
3. Insert new `Assignment` (`InFlight`, `assigned_at = now()`, `deadline_at = now() + 60s`, `worker_id` from request). Set `tasks.status = Assigned`.
4. Read parent job's `script` from disk (`data/{job_id}/*.py`).
5. Commit, return `TaskDispatch`. If no candidate remains after loops, return `204`.

---

### `POST /tasks/{id}/submit`

Submit the result of an in-flight task.

**Request body** matches `SubmitTaskRequest`. **Response `200 OK`** is `{}`. **Errors:** `404` (task/assignment missing), `409` (state/worker/deadline mismatch), `500` (DB failure).

**Side effects**

1. `SELECT ... FOR UPDATE` on the task's most recent assignment row. Under that lock, verify `status = InFlight`, `worker_id` matches, and `deadline_at >= now()`. Any mismatch → `409`. Sequential locking prevents a concurrent `/tasks/next` reclamation from stealing the assignment mid-submit.
2. Mark assignment `Submitted`; persist stdout/stderr/duration_ms. Mark task `Completed`.
3. After commit, count sibling tasks for the same `job_id`. If every sibling is terminal (`Completed` or `Failed`):
   - at least one `Completed` → `UPDATE jobs SET status='completed'`;
   - zero `Completed` (all `Failed`) → `UPDATE jobs SET status='failed'`.

This is the only path that flips Job out of `processing`. The transition `processing → failed` is added to `JobStatus`.

---

### `GET /tasks/stats`

Return current dispatch statistics. **Query:** `job_id: UUID` (required), `worker_id: UUID` (required). **Response `200 OK`** matches `TaskStats`. **Errors:** `404` (no such job), `500` (DB failure).

| Field | Definition |
|-------|------------|
| `pending` | Tasks for `job_id` with `status = Pending`. |
| `in_flight` | Tasks for `job_id` with `status = Assigned` and current assignment `InFlight` AND `deadline_at >= now()`. |
| `completed_total` | Tasks for `job_id` with `status = Completed`. |
| `completed_by_me` | Assignments with `status = Submitted`, belonging to `job_id`, with `worker_id` matching the query. |

**Side effects**

1. `SELECT 1 FROM jobs WHERE id = ?`. If missing, return `404`.
2. Otherwise compute the four counts above and return them.

---

### Frontend: Components

Components live under `modules/task-distribution/ui/`.

| Component | Behavior |
|-----------|----------|
| `StartJobButton` | Props: `upload: UploadCompleted`. Emits: `started: [JobStarted]`, `notify: [Toast]`. Renders a "Process all" button. On click calls `POST /jobs/{id}/start` via module-internal `api.ts`. While the request is in flight, shows a spinner next to the button text and disables the button to prevent duplicate submissions; re-enables on response (success or error). On `200` emits `started` with `{ jobId, taskCount }`. On any failure emits `notify` at `level: "error"`; does NOT emit `started`. |
| `VolunteerRunner` | Props: none. Emits: `notify: [Toast]` on unexpected errors. On mount: reads/creates `workerId` in `localStorage["foinc.worker_id"]` (UUID v4). Periodically polls `POST /tasks/next` when idle, and polls `GET /tasks/stats` while a job is in scope; cadence is an implementation detail tuned for responsiveness without hammering the backend. On a `TaskDispatch`, obtains a `PyodideWorker` via `createPyodideWorker()` from `modules/pyodide-runtime/ui/`, calls `init()` then `exec(task.script, task.input_rows[0].split(","))` (always `input_rows[0]` — Phase 3 always dispatches exactly one row). On exec success, calls `submitTask`. On exec failure, does NOT submit — lets the backend reclaim via deadline. Always terminates the worker when done. Emits `notify` only on unexpected network / 5xx errors from API calls. Tracks the job_id of the most recently picked-up task as its stats-poll target; this may be a job started by another user — in MVP, volunteers are anonymous and pick up whatever is available. |

**Module-internal API client (`modules/task-distribution/ui/api.ts`)**

| Function | Signature | Behavior |
|----------|-----------|----------|
| `startJob` | `(jobId: string, chunkSize?: number) => Promise<StartJobResponse>` | `POST /api/jobs/{jobId}/start`. Throws on non-2xx with body-extracted message. |
| `pollNextTask` | `(workerId: string) => Promise<TaskDispatch \| null>` | `POST /api/tasks/next`. Resolves `null` on `204`, `TaskDispatch` on `200`. Throws on other non-2xx. |
| `submitTask` | `(taskId: string, req: SubmitTaskRequest) => Promise<void>` | `POST /api/tasks/{taskId}/submit`. Resolves on `200`. Throws on non-2xx. |
| `getTaskStats` | `(jobId: string, workerId: string) => Promise<TaskStats>` | `GET /api/tasks/stats?job_id={jobId}&worker_id={workerId}`. Throws on non-2xx. |

**Emitted events (cross-module contract)**

| Component | Event | Payload | Timing |
|-----------|-------|---------|--------|
| `StartJobButton` | `started` | `JobStarted` | Once, after `POST /jobs/{id}/start` returns `200`. |
| `StartJobButton` | `notify` | `Toast` | On any failure from `startJob`. `level: "error"`. |
| `VolunteerRunner` | `notify` | `Toast` | On unexpected network / 5xx errors from any API call. `level: "error"`. |

## Non-goals

- No redundancy, double-dispatch, tiebreaker, or majority-vote — Phase 4 concern.
- No heartbeats / WebSocket push — reclamation is strictly lazy via deadline check.
- `chunk_size > 1` is Phase 4+; Phase 3 dispatches one CSV data row per task. `POST /jobs/{id}/start` ignores any non-1 value; `Task.input_rows` is `Vec<String>` of length exactly 1; `VolunteerRunner` always uses `input_rows[0]`.
- No per-task history UI or progress-over-time aggregation — shows only current stats and the single in-flight task.
- No authentication / worker authorization — any browser picks any `worker_id`; integrity check is the `worker_id`-matches-assignment rule on submit.
- No toast rendering — the shell's `ToastContainer` owns presentation; components only emit `notify`.
