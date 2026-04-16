# Module: Upload

## Purpose

Accept a CSV data file and a Python script via multipart upload, validate file constraints, store both files on disk, persist job metadata in Postgres, and return that metadata to the caller.

## State Machine

Entity: **Job**

| State | Event | Next State | Side effect |
|-------|-------|-----------|-------------|
| *(none)* | `POST /upload` received | `Uploaded` | Validate files; write to `data/{job_id}/`; insert row in `jobs` table; return job metadata. On DB failure after files written, delete `data/{job_id}/` and return `500`. |
| `Uploaded` | Consumed by downstream module | *(out of scope)* | — |

A job has exactly one state within this module: `Uploaded`. Downstream modules (preview, task-distribution) own further transitions.

**Partial-failure semantics:** The `POST /upload` handler executes steps in order: validate, write files, insert DB row. If the DB insert fails after files have been written, the handler deletes the `data/{job_id}/` directory and returns `500`. A failed `POST /upload` produces no entity — either the full sequence succeeds and a job exists, or nothing persists.

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
  "status": "Uploaded",
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

### Internal functions

| Function | Signature | Description | Errors |
|----------|-----------|-------------|--------|
| `validate_upload` | `(csv: &UploadFile, script: &UploadFile) -> Result<(), UploadError>` | Check extensions and sizes against limits | `UploadError::InvalidExtension`, `UploadError::FileTooLarge` |
| `store_files` | `(job_id: Uuid, csv: UploadFile, script: UploadFile) -> Result<(PathBuf, PathBuf), io::Error>` | Write both files to `data/{job_id}/` | `io::Error` |
| `insert_job` | `(pool: &PgPool, job: &Job) -> Result<(), sqlx::Error>` | Insert job metadata row into Postgres | `sqlx::Error` |
| `cleanup_files` | `(job_id: Uuid) -> Result<(), io::Error>` | Delete `data/{job_id}/` directory on partial failure | `io::Error` |
| `get_job` | `(pool: &PgPool, id: Uuid) -> Result<Option<Job>, sqlx::Error>` | Fetch job metadata by ID | `sqlx::Error` |

---

### Frontend: Upload Form

| Component | Behavior |
|-----------|----------|
| `UploadForm` | Drag-and-drop zone or file picker. Accepts exactly one `.csv` and one `.py` file. Calls `POST /upload`. |
| `UploadStatus` | Displays progress during upload (loading state), then shows job metadata on success or error message on failure. |

## Non-goals

- No CSV parsing, preview, or dry-run execution (that is the `preview` module).
- No task splitting or distribution (that is the `task-distribution` module).
- No authentication or authorization (not in MVP scope yet).
- No cloud/S3 storage; files are stored on local disk only.
