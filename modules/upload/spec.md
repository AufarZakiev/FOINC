# Module: Upload

## Purpose

Accept a CSV data file and a Python script via multipart upload, validate file constraints, store both files on disk, persist job metadata in Postgres, return that metadata to the caller, and allow the job to be deleted.

## State Machine

Entity: **Job**

| State | Event | Next State | Side effect |
|-------|-------|-----------|-------------|
| *(none)* | `POST /upload` received | `Uploaded` | Validate files; write to `data/{job_id}/`; insert row in `jobs` table; return job metadata. On DB failure after files written, delete `data/{job_id}/` and return `500`. |
| `Uploaded` | `DELETE /jobs/{id}` received | `Deleted` | Delete `data/{job_id}/` directory, then DELETE the row from `jobs`. Return `204`. |
| *(none)* | `DELETE /jobs/{id}` received | *(none)* | Idempotent: if the row is missing, return `404` without touching disk. |
| `Uploaded` | Consumed by downstream module | *(out of scope)* | — |

A job has two states within this module: `Uploaded` and `Deleted`. `Deleted` is a terminal end-state — the record is removed from the system and subsequent `GET /jobs/{id}` returns `404`. Downstream modules (preview, task-distribution) own further transitions out of `Uploaded`.

**Partial-failure semantics (upload):** The `POST /upload` handler executes steps in order: validate, write files, insert DB row. If the DB insert fails after files have been written, the handler deletes the `data/{job_id}/` directory and returns `500`. A failed `POST /upload` produces no entity — either the full sequence succeeds and a job exists, or nothing persists.

**Partial-failure semantics (delete):** The `DELETE /jobs/{id}` handler deletes files first, then the DB row. If file deletion fails, the handler does NOT touch the DB and returns `500`. If the DB delete fails after files have been removed, the handler logs and returns `500`; state becomes inconsistent but recoverable — the record will `404` on subsequent `GET` and the directory is already gone.

## API / Interface

### Error response schema

All error responses across every endpoint use this body:

```json
{
  "error": "string"
}
```

### Shared types (defined in `integrations/src/`)

| Type | Definition |
|------|------------|
| `JobStatus` | Enum: `Uploaded` (further variants added by downstream modules) |
| `Job` | `job_id: Uuid`, `csv_filename: String`, `script_filename: String`, `csv_size_bytes: i64`, `script_size_bytes: i64`, `status: JobStatus`, `created_at: DateTime<Utc>` |
| `UploadResponse` | `pub type UploadResponse = Job` |

### `POST /upload`

Multipart form-data upload of exactly two files.

**Request (multipart fields)**

| Field | Type | Constraints |
|-------|------|------------|
| `csv_file` | file | Required. Extension `.csv`. Max size 50 MB. |
| `script_file` | file | Required. Extension `.py`. Max size 1 MB. |

**Response `201 Created`**

```json
{
  "job_id": "uuid",
  "csv_filename": "string",
  "script_filename": "string",
  "csv_size_bytes": 0,
  "script_size_bytes": 0,
  "status": "uploaded",
  "created_at": "2026-01-01T00:00:00Z"
}
```

**Error responses**

| Status | Condition |
|--------|-----------|
| `400 Bad Request` | Missing field, wrong extension, or file exceeds size limit |
| `500 Internal Server Error` | Disk write or database failure |

**Side effects**

1. Generate a new UUID v4 `job_id`.
2. Create directory `data/{job_id}/`.
3. Write `data/{job_id}/{original_csv_filename}`.
4. Write `data/{job_id}/{original_script_filename}`.
5. Insert row into `jobs` table.
6. If step 5 fails, delete `data/{job_id}/` and return `500`.

---

### `GET /jobs/{id}`

Return metadata for a single job.

**Path parameters**

| Param | Type | Constraints |
|-------|------|------------|
| `id` | UUID | Required |

**Response `200 OK`**

```json
{
  "job_id": "uuid",
  "csv_filename": "string",
  "script_filename": "string",
  "csv_size_bytes": 0,
  "script_size_bytes": 0,
  "status": "string",
  "created_at": "2026-01-01T00:00:00Z"
}
```

**Error responses**

| Status | Condition |
|--------|-----------|
| `404 Not Found` | No job with the given ID |
| `500 Internal Server Error` | Database failure |

---

### `DELETE /jobs/{id}`

Remove a previously-uploaded job: delete its files on disk and remove its row from the `jobs` table.

**Path parameters**

| Param | Type | Constraints |
|-------|------|------------|
| `id` | UUID | Required |

**Response `204 No Content`**

Empty body.

**Error responses**

| Status | Condition |
|--------|-----------|
| `404 Not Found` | No job with the given ID |
| `500 Internal Server Error` | Filesystem deletion or database failure |

**Side effects**

1. Delete directory `data/{job_id}/` (including both files inside).
2. DELETE the row from `jobs` where `job_id = {id}`.
3. If the row does not exist, return `404` without touching disk.
4. If step 1 fails, do NOT perform step 2; return `500`.
5. If step 2 fails after step 1 succeeded, log the inconsistency and return `500`.

---

### Internal functions

| Function | Signature | Description | Errors |
|----------|-----------|-------------|--------|
| `validate_upload` | `(csv: &UploadFile, script: &UploadFile) -> Result<(), UploadError>` | Check extensions and sizes against limits | `UploadError::InvalidExtension`, `UploadError::FileTooLarge` |
| `store_files` | `(job_id: Uuid, csv: UploadFile, script: UploadFile) -> Result<(PathBuf, PathBuf), io::Error>` | Write both files to `data/{job_id}/` | `io::Error` |
| `insert_job` | `(pool: &PgPool, job: &Job) -> Result<(), sqlx::Error>` | Insert job metadata row into Postgres | `sqlx::Error` |
| `cleanup_files` | `(job_id: Uuid) -> Result<(), io::Error>` | Delete `data/{job_id}/` directory on partial failure or explicit delete | `io::Error` |
| `get_job` | `(pool: &PgPool, id: Uuid) -> Result<Option<Job>, sqlx::Error>` | Fetch job metadata by ID | `sqlx::Error` |
| `delete_job` | `(pool: &PgPool, id: Uuid) -> Result<bool, sqlx::Error>` | DELETE the row from `jobs` by ID. Returns `Ok(true)` if a row was deleted, `Ok(false)` if the row was missing. Pairs with `cleanup_files`: on row-missing, the caller skips IO. | `sqlx::Error` |

---

### Frontend: Upload Form

| Component | Behavior |
|-----------|----------|
| `UploadForm` | Drag-and-drop zone or file picker. Accepts exactly one `.csv` and one `.py` file. Calls `POST /upload`. While the request is in flight, renders an inline loading indicator next to the Upload button and disables the button to prevent duplicate submits. On `201`, reads back the local CSV and script text and emits `uploaded`. On any failure (network error, 4xx, 5xx, or file-read failure), emits `notify` with a `Toast` describing the error; does NOT emit `uploaded`. |

**Emitted by `UploadForm` (cross-module contract):**

| Event | Payload | Timing |
|-------|---------|--------|
| `uploaded` | `UploadCompleted` (see `integrations/ui/events.ts`) | Once, after `POST /upload` returns `201`. Payload carries `jobId` (from response), `script` (raw Python source), `csv` (raw CSV text). Not emitted on error. |
| `notify` | `Toast` (see `integrations/ui/notifications.ts`) | Emitted on upload failure: network error, 4xx, 5xx, or local file-read failure. `level: "error"`, `message` describes the failure. Not emitted on success. |

The upload module does not know who listens to `uploaded` or `notify`, or what they do with them. Composition into any workflow — and toast rendering — is the responsibility of `frontend/`.

**Frontend API (`modules/upload/ui/api.ts`)**

| Function | Signature | Behavior |
|----------|-----------|----------|
| `deleteJob` | `(id: string) => Promise<void>` | Calls `DELETE /api/jobs/{id}`. Resolves on `204 No Content`. Throws on any non-`204` response (including `404` and `500`). Used by the shell (e.g. wizard "Back" button) to discard a previously-uploaded job. |

## Non-goals

- No CSV parsing, preview, dry-run execution, task splitting, or distribution — downstream modules own all behavior past the `uploaded` event.
- No authentication, authorization, or auth check on `DELETE /jobs/{id}` (anyone with a `job_id` may delete), and no cascade deletion of downstream artifacts — downstream modules add their own cleanup.
- No cloud/S3 storage; files are stored on local disk only.
- No toast rendering inside this module — the shell owns the `ToastContainer`; modules only emit `notify`.
