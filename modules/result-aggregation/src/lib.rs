//! Result aggregation: consensus resolution and CSV assembly.
//!
//! This module participates in the task lifecycle via `try_resolve_consensus`
//! (invoked from `task-distribution::submit_task`) and exposes the
//! read-only `GET /jobs/{id}/result` endpoint that streams the assembled
//! CSV back to the scientist.
//!
//! See `modules/result-aggregation/spec.md` for the policy and contract.

pub mod aggregate;
pub mod consensus;
pub mod handlers;
pub mod normalize;

pub use aggregate::{assemble_result, AggregateError};
pub use consensus::{try_resolve_consensus, ConsensusOutcome};
pub use handlers::get_result_handler;
pub use normalize::normalize_stdout;

/// Test-only shared state used across the crate's test modules to
/// serialize DB-touching tests. The task-distribution crate keeps an
/// equivalent lock at `foinc_task_distribution::test_support::DB_LOCK`;
/// that lock is `pub(crate)` and cannot cross crate boundaries, so this
/// crate keeps its own copy. All tests that hit Postgres acquire this
/// lock first so they don't clobber each other's `tasks` / `assignments`
/// fixtures when `cargo test` runs them in parallel.
#[cfg(test)]
pub(crate) mod test_support {
    /// Single process-wide lock serializing DB-touching tests.
    pub(crate) static DB_LOCK: tokio::sync::Mutex<()> =
        tokio::sync::Mutex::const_new(());

    /// Truncate the shared schema so each test starts from a clean slate.
    /// Uses CASCADE to flush `assignments` → `tasks` → `jobs` in one shot.
    pub(crate) async fn reset_db(pool: &sqlx::PgPool) {
        sqlx::query("TRUNCATE TABLE assignments, tasks, jobs CASCADE")
            .execute(pool)
            .await
            .expect("reset_db: TRUNCATE failed");
    }
}
