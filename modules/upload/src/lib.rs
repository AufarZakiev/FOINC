pub mod db;
pub mod handlers;
pub mod storage;
pub mod validation;

pub use handlers::{delete_job_handler, get_job_handler, upload_handler};

/// Test-only shared state used across the crate's test modules to serialize
/// env-var mutations (notably `DATA_DIR`). Both `storage::tests` and
/// `handlers::tests` mutate this env var; without a single shared lock they
/// can race and clobber each other when `cargo test` runs them in parallel.
#[cfg(test)]
pub(crate) mod test_support {
    use std::sync::Mutex;

    /// Single process-wide lock guarding `DATA_DIR` mutations across all
    /// test modules in this crate.
    pub(crate) static ENV_LOCK: Mutex<()> = Mutex::new(());
}
