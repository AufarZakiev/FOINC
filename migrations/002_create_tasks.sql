-- Tasks owned by the task-distribution module.
--
-- Each row represents a single unit of work derived from a parent job's CSV.
-- In Phase 3 the `input_rows` array always has length 1 (one CSV data row
-- per task); `chunk_size > 1` is deferred to a later phase.
CREATE TABLE IF NOT EXISTS tasks (
    task_id      UUID PRIMARY KEY,
    job_id       UUID NOT NULL REFERENCES jobs(job_id) ON DELETE CASCADE,
    chunk_index  INTEGER NOT NULL,
    input_rows   TEXT[] NOT NULL,
    status       TEXT NOT NULL,
    attempts     INTEGER NOT NULL DEFAULT 0,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT tasks_status_check
        CHECK (status IN ('pending', 'assigned', 'completed', 'failed'))
);

-- Hot path: `POST /tasks/next` scans for Pending rows, optionally filtered
-- by job. Composite (status, job_id) matches that access pattern without
-- forcing the planner to read the whole table.
CREATE INDEX IF NOT EXISTS tasks_status_job_id_idx
    ON tasks (status, job_id);

-- Assignments: one row per dispatch of a task to a worker. Tasks can
-- accumulate multiple rows across retries; the "current" assignment is the
-- one with the latest `assigned_at` (or equivalently, the unique InFlight
-- row).
CREATE TABLE IF NOT EXISTS assignments (
    assignment_id  UUID PRIMARY KEY,
    task_id        UUID NOT NULL REFERENCES tasks(task_id) ON DELETE CASCADE,
    worker_id      UUID NOT NULL,
    assigned_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    deadline_at    TIMESTAMPTZ NOT NULL,
    status         TEXT NOT NULL,
    stdout         TEXT,
    stderr         TEXT,
    duration_ms    DOUBLE PRECISION,
    CONSTRAINT assignments_status_check
        CHECK (status IN ('in_flight', 'submitted', 'timed_out'))
);

-- Hot path: locating the current assignment for a task, either during
-- submit ("latest assignment for this task") or during reclamation ("is
-- this task's current InFlight row past deadline?"). (task_id, status)
-- supports both, and the planner can fall back to a sort on assigned_at
-- when needed.
CREATE INDEX IF NOT EXISTS assignments_task_id_status_idx
    ON assignments (task_id, status);
