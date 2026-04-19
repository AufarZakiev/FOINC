pub mod csv_split;
pub mod db;
pub mod handlers;

pub use handlers::{
    next_task_handler, start_job_handler, submit_task_handler, task_stats_handler,
};

/// Test-only shared state used across the crate's test modules to serialize
/// env-var mutations (notably `DATA_DIR`). The upload crate defines an
/// identical lock at `foinc_upload::test_support::ENV_LOCK`; that lock is
/// `pub(crate)` and can't be reused across crate boundaries, so the
/// task-distribution crate keeps its own copy.
#[cfg(test)]
pub(crate) mod test_support {
    use std::sync::Mutex;

    /// Single process-wide lock guarding `DATA_DIR` mutations across all
    /// test modules in this crate.
    pub(crate) static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// Single process-wide lock serializing DB-touching tests. Tests in this
    /// crate share a single Postgres instance and the pick/reclaim logic
    /// scans the global `tasks` table — parallel tests would otherwise see
    /// each other's fixtures. Acquiring this lock at the start of every
    /// DB-touching test keeps them deterministic at the cost of no-parallel
    /// speedup. The lock is an async-friendly `tokio::sync::Mutex` so it
    /// never blocks the worker thread.
    pub(crate) static DB_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    /// Truncate the task-distribution tables so each test starts from a
    /// clean slate. Runs inside a single transaction to avoid partial
    /// cleanup on failure.
    pub(crate) async fn reset_db(pool: &sqlx::PgPool) {
        // CASCADE propagates through the FK from assignments -> tasks, and
        // from tasks -> jobs, so truncating jobs wipes everything.
        sqlx::query("TRUNCATE TABLE assignments, tasks, jobs CASCADE")
            .execute(pool)
            .await
            .expect("reset_db: TRUNCATE failed");
    }
}
