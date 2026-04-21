-- Phase 4: consensus / result-aggregation schema additions.
--
-- Adds redundancy bookkeeping and the `awaiting_consensus` task status so
-- that redundantly-dispatched tasks can be compared via SHA-256 of
-- normalized stdout.
--
-- * `tasks.redundancy_target` — how many distinct-worker Submitted
--   assignments must exist before consensus is attempted. Default 2 for
--   the standard duplicate-and-compare policy; may be overridden at
--   insert-time by the `FOINC_REDUNDANCY` env var (see the
--   task-distribution spec).
-- * `tasks.winning_assignment_id` — FK to the assignment whose stdout is
--   chosen as the authoritative result; set by result-aggregation when
--   consensus resolves to Completed.
-- * `assignments.result_hash` — SHA-256 hex of `normalize_stdout(stdout)`
--   populated at submission time. Consensus compares hashes, not stdouts.
-- * The `tasks_status_check` CHECK is dropped and re-created so the new
--   `awaiting_consensus` variant is accepted by Postgres.
-- * A composite index on `assignments (task_id, result_hash)` accelerates
--   the `GROUP BY result_hash` query consensus performs per task.

ALTER TABLE tasks
    ADD COLUMN redundancy_target SMALLINT NOT NULL DEFAULT 2;

ALTER TABLE tasks
    ADD COLUMN winning_assignment_id UUID NULL
        REFERENCES assignments(assignment_id) ON DELETE SET NULL;

ALTER TABLE assignments
    ADD COLUMN result_hash TEXT NULL;

ALTER TABLE tasks
    DROP CONSTRAINT tasks_status_check;

ALTER TABLE tasks
    ADD CONSTRAINT tasks_status_check
    CHECK (status IN ('pending', 'assigned', 'awaiting_consensus', 'completed', 'failed'));

CREATE INDEX IF NOT EXISTS assignments_task_id_result_hash_idx
    ON assignments (task_id, result_hash);
