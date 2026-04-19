CREATE TABLE IF NOT EXISTS jobs (
    job_id            UUID PRIMARY KEY,
    csv_filename      TEXT NOT NULL,
    script_filename   TEXT NOT NULL,
    csv_size_bytes    BIGINT NOT NULL,
    script_size_bytes BIGINT NOT NULL,
    status            TEXT NOT NULL,
    created_at        TIMESTAMPTZ NOT NULL
);
