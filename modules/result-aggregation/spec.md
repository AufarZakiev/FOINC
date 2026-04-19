# Module: Result Aggregation

## Purpose

Resolve consensus among redundantly-computed task submissions and assemble the winning per-task stdouts into a single CSV downloadable via `GET /jobs/{id}/result`.

## State Machine

This module owns no tables and no new entities. It participates in two existing transitions owned by `task-distribution`:

- `Task: AwaitingConsensus → Completed` (sets `tasks.winning_assignment_id`)
- `Task: AwaitingConsensus → Failed`

Both transitions are driven exclusively by `try_resolve_consensus`, invoked from `task-distribution::submit_task`. The caller owns all downstream effects (job-terminality recompute); this module only touches `tasks.status`, `tasks.winning_assignment_id`, and `tasks.redundancy_target`.

### Consensus decision table

Input: the set of `Submitted` assignments for the task, each carrying a `result_hash`.

| Submissions | Distinct hashes | Outcome | `tasks.status` | `tasks.winning_assignment_id` | `tasks.redundancy_target` |
|-------------|-----------------|---------|----------------|-------------------------------|---------------------------|
| 2 | 1 | `Completed` | `Completed` | any matching assignment | unchanged (2) |
| 2 | 2 | `Escalated` | unchanged (`AwaitingConsensus`) | unchanged (NULL) | `2 → 3` |
| 3 | 1 or 2 (some hash has ≥2 matches) | `Completed` | `Completed` | any assignment with the ≥2-match hash | unchanged (3) |
| 3 | 3 | `Failed` | `Failed` | unchanged (NULL) | unchanged (3) |
| >3 (defensive; should not occur once resolved) | any (some hash has ≥2 matches) | `Completed` | `Completed` | any assignment with the ≥2-match hash | unchanged |

`Escalated` is only reachable when submissions=2 and the two hashes disagree; the bump from `2 → 3` is therefore applied at most once per task. `pick_next_task` naturally dispatches the task to a third worker on its next invocation because `redundancy_target` now exceeds effective dispatches.

## API / Interface

### Error response schema

All error responses use `{ "error": "string" }`.

### Shared types

This module introduces no new types in `integrations/`. It references:

- `JobStatus` (from `integrations/src/lib.rs` and `integrations/ui/types.ts`) — no new variant.
- `Job` (from `integrations/src/lib.rs`) — read-only, to determine response code.

`ConsensusOutcome` is module-internal (Rust-only) and does not cross the HTTP or UI boundary.

### `GET /jobs/{id}/result`

Download the assembled CSV for a completed job.

**Path parameters**

| Param | Type | Constraints |
|-------|------|-------------|
| `id` | UUID | Required |

**Response `200 OK`**

- `Content-Type: text/csv; charset=utf-8`
- `Content-Disposition: attachment; filename="job-<short-id>.csv"` where `<short-id>` is the first 8 hex characters of `job_id`
- Body: concatenation of each task's winning assignment's RAW stdout (unmodified, not re-normalized), ordered by `tasks.chunk_index` ascending, joined with a single `\n` separator between tasks. No mandated leading or trailing newline — whatever the script emitted is preserved.

**Error responses**

| Status | Condition | Body |
|--------|-----------|------|
| `404 Not Found` | No job with the given `id` | `{"error": "Job not found"}` |
| `409 Conflict` | Job `status` is `uploaded` or `processing` | `{"error": "Job not yet complete"}` |
| `422 Unprocessable Entity` | Job `status` is `failed` | `{"error": "Job failed"}` |
| `500 Internal Server Error` | DB or IO failure | `{"error": "..."}` |

**Side effects**

Read-only. No status changes, no writes, no caching. The CSV is re-assembled on every request.

### Internal functions

| Function | Signature | Description | Errors |
|----------|-----------|-------------|--------|
| `try_resolve_consensus` | `(tx: &mut sqlx::Transaction<'_, sqlx::Postgres>, task_id: Uuid) -> Result<ConsensusOutcome, sqlx::Error>` | Consensus policy hand-off invoked from `task-distribution::submit_task`. Runs WITHIN the caller's transaction — does NOT commit. See side effects below. | `sqlx::Error` |
| `normalize_stdout` | `(s: &str) -> String` | Canonicalize stdout for hashing: rstrip each line (remove trailing whitespace), drop trailing blank lines. Pure, deterministic. | — |
| `assemble_result` | `(pool: &PgPool, job_id: Uuid) -> Result<String, AggregateError>` | Read every task's winning assignment for `job_id`, order by `tasks.chunk_index`, concatenate raw `stdout` joined by `\n`. | `AggregateError::NotFound`, `AggregateError::NotComplete`, `AggregateError::Failed`, `sqlx::Error` |

`ConsensusOutcome`: module-internal enum `Completed | Escalated | Failed`.

`AggregateError`: module-internal enum mapped to HTTP codes by the handler (`NotFound → 404`, `NotComplete → 409`, `Failed → 422`, `sqlx::Error → 500`).

#### `try_resolve_consensus` side effects

Executed in order, all inside the caller's transaction:

1. `SELECT ... FOR UPDATE` on `tasks` where `task_id = ?` to serialize with any concurrent writer.
2. Read `result_hash` for every `Submitted` assignment of this task; group by hash and count.
3. Apply the consensus decision table above.
4. If `Completed`: `UPDATE tasks SET status='completed', winning_assignment_id=<any assignment_id whose result_hash is the majority hash> WHERE task_id=?`. Return `ConsensusOutcome::Completed`.
5. If `Escalated`: `UPDATE tasks SET redundancy_target = redundancy_target + 1 WHERE task_id=?`. Leave `tasks.status` as `awaiting_consensus`. Return `ConsensusOutcome::Escalated`.
6. If `Failed`: `UPDATE tasks SET status='failed' WHERE task_id=?`. Return `ConsensusOutcome::Failed`.

Does NOT recompute job-level terminality — the caller (`submit_task`) re-reads `tasks.status` and applies the Phase-4 job rule after the hook returns.

#### `normalize_stdout` contract

`normalize_stdout` is called at TWO distinct points:

- By `task-distribution::submit_task` to compute `result_hash = sha256_hex(normalize_stdout(stdout))` before persisting the assignment. `result_hash` is what consensus compares.
- It is NOT applied to the aggregation endpoint's body. `assemble_result` emits the winning assignment's RAW stdout so the scientist's formatting (trailing newlines, spacing) is preserved.

### Frontend

Components live under `modules/result-aggregation/ui/`.

| Component | Behavior |
|-----------|----------|
| `DownloadResultButton` | Props: `jobId: string`, `jobStatus: JobStatus`. Emits: none. Renders a primary-styled download button with `href="/api/jobs/{jobId}/result"` and the native `download` attribute ONLY when `jobStatus === "completed"`. For any other status renders nothing (empty fragment). No JS fetch, no click handler — the browser handles the streaming download and filename from `Content-Disposition`. |

The component performs no API calls itself; it is a pure conditional anchor. No toast emission, no error state — if the anchor is rendered, the backend has already attested that the job is `completed`, and a subsequent download failure (e.g. 500) is surfaced by the browser's own download UI.

## Non-goals

- No progress UI, partial downloads, or streaming-while-computing — a download is available only after the job is fully `completed`. Progress is Phase 5 (`progress-tracking`).
- No ETag, `Last-Modified`, or any caching — the CSV is recomputed on every request.
- No format conversion, no column rewriting, no header injection — the body is raw concatenated stdout.
- No authentication or authorization — any client holding the `jobId` may download.
- No consensus policy beyond the hash-equality decision table — no weighted voting, no trust scores, no reputation.
- No retention or cleanup — the module never deletes assignments or tasks.
