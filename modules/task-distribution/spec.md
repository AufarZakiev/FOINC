# Module: Task Distribution

## Purpose
Split an uploaded job's CSV into per-row tasks, dispatch them redundantly to browser-based volunteer workers with deadline-based reclamation, collect results, and surface live progress to volunteers.

## Database schema (module-internal)

`tasks(task_id, job_id, chunk_index, input_rows, status, attempts, redundancy_target, winning_assignment_id, created_at)` and `assignments(assignment_id, task_id, worker_id, assigned_at, deadline_at, status, stdout, stderr, duration_ms, result_hash)`. Both tables are owned by this module; no other module reads or writes them. `input_rows` is a text array of length exactly 1 in Phase 4 (see non-goals).

Phase 4 additions (delivered via `migrations/004_add_consensus_fields.sql`):
- `tasks.redundancy_target SMALLINT NOT NULL DEFAULT 2` — how many distinct-worker `Submitted` assignments must exist before consensus is attempted.
- `tasks.winning_assignment_id UUID NULL REFERENCES assignments(assignment_id)` — set by the result-aggregation hand-off when consensus resolves to `Completed`; NULL otherwise.
- `assignments.result_hash TEXT NULL` — SHA-256 hex of normalized stdout (rstrip per line, drop trailing blank lines), set at submission time.

## Configuration

`FOINC_REDUNDANCY` (backend env var, read at process start). If set to a positive integer, overrides the default value used when inserting new `Pending` task rows in `POST /jobs/{id}/start`. Default = 2 if unset. Useful for local single-volunteer demos (`FOINC_REDUNDANCY=1`). Not a wire contract; never appears in HTTP payloads.

## State Machine

### Entity: Task

| State | Event | → State | Side effect |
|-------|-------|---------|-------------|
| *(none)* | `POST /jobs/{id}/start` received | `Pending` | Insert row with `attempts = 0`, `redundancy_target = $FOINC_REDUNDANCY or 2`. |
| `Pending` | Dispatched via `POST /tasks/next` (first worker) | `Assigned` | Insert `Assignment` (`InFlight`, `deadline_at = now() + 60s`). `attempts` NOT incremented. |
| `Assigned` | `POST /tasks/{id}/submit` accepted; `Submitted` count after commit `< redundancy_target` | `AwaitingConsensus` | Mark assignment `Submitted`; store stdout/stderr/duration_ms/result_hash. |
| `Assigned` | `POST /tasks/{id}/submit` accepted; `Submitted` count after commit `>= redundancy_target` | *(delegated)* | Hand off to `result_aggregation::try_resolve_consensus` (see below). |
| `AwaitingConsensus` | Dispatched via `POST /tasks/next` to an additional worker | `AwaitingConsensus` | Insert new `Assignment` (`InFlight`); task status unchanged. |
| `AwaitingConsensus` | `POST /tasks/{id}/submit` accepted; `Submitted` count after commit `< redundancy_target` | `AwaitingConsensus` | Persist submission; no status change. |
| `AwaitingConsensus` | `POST /tasks/{id}/submit` accepted; `Submitted` count after commit `>= redundancy_target` | *(delegated)* | Hand off to `result_aggregation::try_resolve_consensus`. May flip task to `Completed` (sets `winning_assignment_id`) or `Failed` (all hashes disagree). |
| `Assigned` / `AwaitingConsensus` | `POST /tasks/next` sees oldest live `InFlight` assignment expired AND `attempts < 5` | *(unchanged)* | Increment `attempts`, mark old assignment `TimedOut`. Task status unchanged; a fresh candidate pick in the same loop may dispatch the new assignment. Attempts counter ONLY increments on timeout reclamation. |
| `Assigned` / `AwaitingConsensus` | `POST /tasks/next` sees expired assignment AND `attempts >= 5` | `Failed` | Mark that assignment `TimedOut`, mark task `Failed`, do NOT redispatch. |
| `Assigned` / `AwaitingConsensus` | Script/CSV files missing at dispatch | `Failed` | Mark just-inserted assignment `TimedOut`; mark task `Failed`; `attempts` NOT incremented (unrecoverable). |

Reclamation is lazy: only the next `POST /tasks/next` notices a missed deadline. Timeout reclamation increments `attempts` but does NOT transition the task status out of `AwaitingConsensus`; the task remains eligible for further dispatch until either consensus resolves or the retry budget is exhausted.

### Entity: Assignment

| State | Event | → State | Side effect |
|-------|-------|---------|-------------|
| *(none)* | Task dispatched via `POST /tasks/next` | `InFlight` | Row created with `assigned_at = now()`, `deadline_at = now() + 60s`, `result_hash = NULL`. |
| `InFlight` | Matching `POST /tasks/{id}/submit` received, deadline not passed | `Submitted` | Persist stdout/stderr/duration_ms and compute `result_hash`. |
| `InFlight` | Picker observes `deadline_at < now()` on reclamation path | `TimedOut` | Parent task transitions per table above. |

Unlisted transitions are invalid.

### Entity: Job (terminality, Phase 4 — breaking change)

Applied by `submit_task` (post-consensus) and `fail_task` (orphan path):
- Job → `completed` iff **ALL** sibling tasks are `Completed`.
- Job → `failed` iff at least one sibling is `Failed` AND every other sibling is terminal (`Completed` or `Failed`).
- Otherwise Job stays `processing`.

This tightens the prior Phase 3 rule ("≥1 `Completed` is enough"). Both flip paths (`/submit` and `/next` orphan-fail) apply this rule identically.

## API / Interface

### Error response schema

All error responses use `{ "error": "string" }`.

### Shared types (defined in `integrations/src/`)

| Type | Definition |
|------|------------|
| `JobStatus` | Enum `Uploaded`, `Processing`, `Completed`, `Failed`. Wire: snake_case. |
| `TaskStatus` | Enum `Pending`, `Assigned`, `AwaitingConsensus`, `Completed`, `Failed`. Wire: snake_case (`"awaiting_consensus"`). |
| `AssignmentStatus` | Enum `InFlight`, `Submitted`, `TimedOut`. |
| `StartJobRequest` | `chunk_size: Option<u32>` (ignored in Phase 4). |
| `StartJobResponse` | `job_id: Uuid`, `task_count: u32`. |
| `NextTaskRequest` | `worker_id: Uuid`. |
| `TaskDispatch` | `task_id: Uuid`, `job_id: Uuid`, `script: String`, `input_rows: Vec<String>` (len 1), `deadline_at: DateTime<Utc>`. |
| `SubmitTaskRequest` | `worker_id: Uuid`, `stdout: String`, `stderr: String`, `duration_ms: f64`. |
| `TaskStats` | `pending: i64`, `in_flight: i64`, `awaiting_consensus: i64`, `completed_total: i64`, `completed_by_me: i64`. |

`Task` and `Assignment` are module-internal DB shapes and are NOT in `integrations/`.

### Shared UI types (defined in `integrations/ui/`)

| File | Addition |
|------|----------|
| `integrations/ui/types.ts` | `JobStatus` union `"uploaded" \| "processing" \| "completed" \| "failed"`. `TaskDispatch` mirrors Rust. `TaskStats` TS shape: `{ pending: number; in_flight: number; awaiting_consensus: number; completed_total: number; completed_by_me: number }` — snake_case on the wire. |
| `integrations/ui/events.ts` | `JobStarted { jobId: string; taskCount: number }`; emitter `StartJobButton` after `POST /jobs/{id}/start` success; consumer is the shell. |

### `POST /jobs/{id}/start`

Split a previously uploaded job's CSV into tasks and move the job to `processing`.

**Path:** `id: UUID`, must reference a job in state `uploaded`. **Request** matches `StartJobRequest`. **Response `200 OK`** matches `StartJobResponse`. **Errors:** `404` (no such job), `409` (not `uploaded`), `500` (CSV/DB failure).

**Side effects**
1. Atomic check-and-set: `UPDATE jobs SET status='processing' WHERE id=? AND status='uploaded' RETURNING ...`. Concurrent losers return `409`.
2. Winner reads `data/{job_id}/*.csv`, splits on `\n`, trims, drops empties, drops the first remaining line (header).
3. Inserts one `tasks` row per data line with `status = Pending`, `attempts = 0`, `redundancy_target = FOINC_REDUNDANCY or 2`, `input_rows = [line]`.
4. Returns `{ job_id, task_count }`.

---

### `POST /tasks/next`

Atomically pick the next available task for the calling worker, respecting redundancy and no-double-dispatch-to-same-worker.

**Request** matches `NextTaskRequest`. **Response `200 OK`** matches `TaskDispatch`; **`204 No Content`** when nothing eligible. **Errors:** `500` (DB failure).

**Candidate rule.** A task is a candidate iff:
- `status IN ('pending', 'awaiting_consensus')`, AND
- `(count of Submitted assignments) + (count of InFlight assignments with deadline_at >= now())` (= "effective dispatches") `< redundancy_target`, AND
- No assignment (any status) exists for `(task_id, worker_id = $request.worker_id)` (no double-dispatch to the same worker).

**Side effects (single DB transaction per pick attempt; handler may loop across multiple transactions)**
1. Candidate query uses `SELECT ... FOR UPDATE SKIP LOCKED` on the `tasks` row. Candidates match the rule above; expired `InFlight` assignments are detected via a correlated subquery over `assignments`. Locking the task row serializes timeout reclamation + new-assignment creation.
2. For each expired `InFlight` assignment on the locked task: evaluate `tasks.attempts >= 5` FIRST. If true, mark that assignment `TimedOut`, mark task `Failed`, commit, loop. Otherwise increment `attempts`, mark assignment `TimedOut`, proceed. (Expired assignments are no longer counted toward effective dispatches.)
3. Re-evaluate the "effective dispatches < redundancy_target" predicate under the row lock; also re-check the no-double-dispatch rule for the caller. If either fails, release the lock and loop. Otherwise insert a new `Assignment` (`InFlight`, `deadline_at = now() + 60s`, caller's `worker_id`). If task was `Pending`, flip to `Assigned`; if `AwaitingConsensus`, keep it. Commit.
4. **Orphan-recovery (post-commit).** Read `data/{job_id}/*.py` and `data/{job_id}/*.csv`. On success build `TaskDispatch`, return 200. On any IO error invoke `fail_task(pool, task_id)` (marks just-inserted assignment `TimedOut`, marks task `Failed` without incrementing `attempts`, applies the Phase-4 job-terminality rule), then loop.
5. Loop exits on a readable task (return `200 + TaskDispatch`) or when no candidate remains (return `204`).

**Note.** This endpoint flips `jobs.status` out of `processing` via the orphan-fail path in step 4, applying the Phase-4 job-terminality rule (same rule as `/submit`).

---

### `POST /tasks/{id}/submit`

Submit the result of an in-flight assignment; may trigger consensus.

**Request** matches `SubmitTaskRequest`. **Response `200 OK`** is `{}`. **Errors:** `404`, `409`, `500`.

**Side effects**
1. `SELECT ... FOR UPDATE` on the caller's most recent `InFlight` assignment for this task (matched by `worker_id`). Under the lock, verify `status = InFlight`, `worker_id` matches, `deadline_at >= now()`. Any mismatch → `409`.
2. Compute `result_hash = sha256_hex(normalize(stdout))` where `normalize` = rstrip each line, drop trailing blank lines. Mark assignment `Submitted`; persist stdout/stderr/duration_ms/result_hash.
3. Count `Submitted` assignments for this task (after commit of step 2 semantics, within the same tx). If `count < redundancy_target`: set `tasks.status = 'awaiting_consensus'` (idempotent; no-op if already there), commit. Skip step 4.
4. Otherwise (`count >= redundancy_target`): call `result_aggregation::try_resolve_consensus(&mut tx, task_id)` (same transaction). The hook reads the task's `Submitted` assignments, applies its majority/tiebreaker policy, and may update `tasks.status` to `Completed` (setting `winning_assignment_id`) or `Failed`, or leave it in `AwaitingConsensus` if policy deems the set still inconclusive. Commit.
5. After commit, recompute sibling terminality for `job_id` using the Phase-4 rule (see Entity: Job). Apply to `jobs.status` only if a terminal rule matches.

`/submit` is one of two paths that flips Job out of `processing`; the other is the orphan-fail path in `POST /tasks/next` step 4. Both apply the same rule.

---

### `GET /tasks/stats`

**Query:** `job_id: UUID` (required), `worker_id: UUID` (required). **Response `200 OK`** matches `TaskStats`. **Errors:** `404` (no such job), `500`.

| Field | Definition |
|-------|------------|
| `pending` | Tasks for `job_id` with `status = Pending`. |
| `in_flight` | Tasks for `job_id` with `status IN (Assigned, AwaitingConsensus)` AND at least one `InFlight` assignment whose `deadline_at >= now()`. |
| `awaiting_consensus` | Tasks for `job_id` with `status = AwaitingConsensus`. |
| `completed_total` | Tasks for `job_id` with `status = Completed`. |
| `completed_by_me` | Assignments `Submitted`, `job_id` match, `worker_id` match. |

Handler verifies job existence, else `404`.

---

### Internal functions

| Function | Signature | Description | Errors |
|----------|-----------|-------------|--------|
| `pick_next_task` | `(pool: &PgPool, worker_id: Uuid) -> Result<Option<(Uuid, Uuid, Vec<String>, DateTime<Utc>)>, sqlx::Error>` | Implements the candidate rule + side-effect steps 1-3 of `POST /tasks/next`. Returns `(task_id, job_id, input_rows, deadline_at)` or `None`. Does NOT read files. | `sqlx::Error` |
| `submit_task` | `(pool: &PgPool, task_id: Uuid, req: &SubmitTaskRequest) -> Result<JobTerminalState, SubmitError>` | Implements `/submit` side effects 1-5, including the `try_resolve_consensus` hand-off and Phase-4 job-terminality recompute. | `SubmitError::NotFound`, `SubmitError::Conflict`, `sqlx::Error` |
| `fail_task` | `(pool: &PgPool, task_id: Uuid) -> Result<JobTerminalState, sqlx::Error>` | Marks the task's most recent `InFlight` assignment `TimedOut`, marks task `Failed`, does NOT increment `attempts`. Applies Phase-4 job-terminality rule. Used by orphan-recovery. | `sqlx::Error` |

`JobTerminalState`: module-internal enum `StillProcessing | FlippedCompleted | FlippedFailed`.

### Cross-module hand-off

`result_aggregation::try_resolve_consensus` is exposed by the `result-aggregation` crate and called from `submit_task` step 4. Expected signature:

```
pub async fn try_resolve_consensus(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    task_id: uuid::Uuid,
) -> Result<ConsensusOutcome, sqlx::Error>;
```

`ConsensusOutcome` is owned by result-aggregation; this module treats it opaquely and re-reads `tasks.status` after the call to decide the job-terminality recompute. The function is responsible for any updates to `tasks.status`, `tasks.winning_assignment_id`, and for reading `assignments.result_hash` values. This spec does not describe its policy.

---

### Frontend: Components

Components live under `modules/task-distribution/ui/`.

| Component | Behavior |
|-----------|----------|
| `StartJobButton` | Props: `upload: UploadCompleted`. Emits: `started: [JobStarted]`, `notify: [Toast]`. Button calls `POST /jobs/{id}/start`; disables while in flight. On `200` emits `started`. On failure emits `notify` at `level: "error"`. |
| `VolunteerRunner` | Props: none. Emits: `notify: [Toast]` on unexpected errors. Reads/creates `workerId` in `localStorage["foinc.worker_id"]`. Polls `POST /tasks/next` when idle, `GET /tasks/stats` while a job is in scope. On `TaskDispatch`: obtain `PyodideWorker` via `createPyodideWorker()` (from `modules/pyodide-runtime/ui/`), `init()` then `exec(task.script, task.input_rows[0].split(","))`. On success, `submitTask`. On exec failure, does NOT submit — backend reclaims via deadline. Always terminates the worker when done. Tracks last picked task's `job_id` as stats-poll target. |

**Module-internal API client (`modules/task-distribution/ui/api.ts`)**

| Function | Signature | Behavior |
|----------|-----------|----------|
| `startJob` | `(jobId, chunkSize?) => Promise<StartJobResponse>` | `POST /api/jobs/{jobId}/start`. Throws on non-2xx. |
| `pollNextTask` | `(workerId) => Promise<TaskDispatch \| null>` | `POST /api/tasks/next`. `null` on `204`. |
| `submitTask` | `(taskId, req) => Promise<void>` | `POST /api/tasks/{taskId}/submit`. |
| `getTaskStats` | `(jobId, workerId) => Promise<TaskStats>` | `GET /api/tasks/stats?...`. |

**Emitted events (cross-module contract)**

| Component | Event | Payload | Timing |
|-----------|-------|---------|--------|
| `StartJobButton` | `started` | `JobStarted` | After `POST /jobs/{id}/start` returns `200`. |
| `StartJobButton` | `notify` | `Toast` | On any `startJob` failure. `level: "error"`. |
| `VolunteerRunner` | `notify` | `Toast` | On unexpected network / 5xx errors. `level: "error"`. |

## Non-goals

- Consensus policy (majority vote, tiebreaker dispatch, quorum sizing) lives in `result-aggregation`; this module only hands off via `try_resolve_consensus`.
- No heartbeats / WebSocket push — reclamation is strictly lazy.
- `chunk_size > 1` is out of scope; Phase 4 still dispatches one CSV row per task. `POST /jobs/{id}/start` ignores any non-1 value.
- No per-task history UI or progress-over-time aggregation.
- No authentication / worker authorization — integrity check is the `worker_id`-matches-assignment rule on submit plus no-double-dispatch at pick time.
- No toast rendering — the shell's `ToastContainer` owns presentation.
