use crate::validation::UploadFile;
use std::io;
use std::path::PathBuf;
use uuid::Uuid;

/// Base directory for file storage. Can be overridden via the `DATA_DIR` env var.
fn data_dir() -> String {
    std::env::var("DATA_DIR").unwrap_or_else(|_| "data".to_string())
}

/// Write both uploaded files to `data/{job_id}/` using their original filenames.
///
/// Creates the `data/{job_id}/` directory and writes the CSV and script files
/// into it. Returns the paths to the written files on success.
pub async fn store_files(
    job_id: Uuid,
    csv: UploadFile,
    script: UploadFile,
) -> Result<(PathBuf, PathBuf), io::Error> {
    let dir = PathBuf::from(data_dir()).join(job_id.to_string());
    tokio::fs::create_dir_all(&dir).await?;

    let csv_path = dir.join(&csv.filename);
    tokio::fs::write(&csv_path, &csv.data).await?;

    let script_path = dir.join(&script.filename);
    tokio::fs::write(&script_path, &script.data).await?;

    Ok((csv_path, script_path))
}

/// Delete the `data/{job_id}/` directory and all its contents.
///
/// Used to clean up files when the database insert fails after files have
/// already been written to disk.
pub async fn cleanup_files(job_id: Uuid) -> Result<(), io::Error> {
    let dir = PathBuf::from(data_dir()).join(job_id.to_string());
    tokio::fs::remove_dir_all(&dir).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::sync::Mutex;

    /// Guard to serialize storage tests that mutate the DATA_DIR env var.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// RAII guard that sets DATA_DIR to a unique temp directory on creation
    /// and cleans up (removes the env var + deletes the temp directory) on drop.
    /// This guarantees cleanup even if a test panics.
    struct DataDirGuard {
        dir: PathBuf,
        _lock: std::sync::MutexGuard<'static, ()>,
    }

    impl DataDirGuard {
        fn new() -> Self {
            let lock = ENV_LOCK.lock().unwrap();
            let dir = std::env::temp_dir().join(format!("foinc_test_{}", Uuid::new_v4()));
            std::fs::create_dir_all(&dir).unwrap();
            std::env::set_var("DATA_DIR", dir.to_str().unwrap());
            Self { dir, _lock: lock }
        }

        fn path(&self) -> &Path {
            &self.dir
        }
    }

    impl Drop for DataDirGuard {
        fn drop(&mut self) {
            std::env::remove_var("DATA_DIR");
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn make_file(name: &str, content: &[u8]) -> UploadFile {
        UploadFile {
            filename: name.to_string(),
            data: content.to_vec(),
        }
    }

    // --- store_files tests ---

    #[tokio::test]
    async fn test_store_files_creates_job_directory() {
        let guard = DataDirGuard::new();
        let job_id = Uuid::new_v4();
        let csv = make_file("input.csv", b"a,b,c");
        let script = make_file("run.py", b"print('hi')");

        let result = store_files(job_id, csv, script).await;
        assert!(result.is_ok());

        let job_dir = guard.path().join(job_id.to_string());
        assert!(job_dir.is_dir());
    }

    #[tokio::test]
    async fn test_store_files_writes_csv_with_correct_content() {
        let _guard = DataDirGuard::new();
        let job_id = Uuid::new_v4();
        let csv_content = b"col1,col2\n1,2\n3,4";
        let csv = make_file("data.csv", csv_content);
        let script = make_file("script.py", b"pass");

        let (csv_path, _) = store_files(job_id, csv, script).await.unwrap();

        let written = std::fs::read(&csv_path).unwrap();
        assert_eq!(written, csv_content);
    }

    #[tokio::test]
    async fn test_store_files_writes_script_with_correct_content() {
        let _guard = DataDirGuard::new();
        let job_id = Uuid::new_v4();
        let script_content = b"import pandas\ndf = pandas.read_csv('x.csv')";
        let csv = make_file("data.csv", b"x");
        let script = make_file("analysis.py", script_content);

        let (_, script_path) = store_files(job_id, csv, script).await.unwrap();

        let written = std::fs::read(&script_path).unwrap();
        assert_eq!(written, script_content);
    }

    #[tokio::test]
    async fn test_store_files_uses_original_csv_filename() {
        let _guard = DataDirGuard::new();
        let job_id = Uuid::new_v4();
        let csv = make_file("my_data.csv", b"a");
        let script = make_file("run.py", b"b");

        let (csv_path, _) = store_files(job_id, csv, script).await.unwrap();

        assert_eq!(csv_path.file_name().unwrap(), "my_data.csv");
    }

    #[tokio::test]
    async fn test_store_files_uses_original_script_filename() {
        let _guard = DataDirGuard::new();
        let job_id = Uuid::new_v4();
        let csv = make_file("data.csv", b"a");
        let script = make_file("my_analysis.py", b"b");

        let (_, script_path) = store_files(job_id, csv, script).await.unwrap();

        assert_eq!(script_path.file_name().unwrap(), "my_analysis.py");
    }

    #[tokio::test]
    async fn test_store_files_returns_paths_under_job_directory() {
        let guard = DataDirGuard::new();
        let job_id = Uuid::new_v4();
        let csv = make_file("data.csv", b"x");
        let script = make_file("run.py", b"y");

        let (csv_path, script_path) = store_files(job_id, csv, script).await.unwrap();

        let expected_dir = guard.path().join(job_id.to_string());
        assert_eq!(csv_path.parent().unwrap(), expected_dir);
        assert_eq!(script_path.parent().unwrap(), expected_dir);
    }

    // --- cleanup_files tests ---

    #[tokio::test]
    async fn test_cleanup_files_removes_job_directory() {
        let guard = DataDirGuard::new();
        let job_id = Uuid::new_v4();
        let csv = make_file("data.csv", b"x");
        let script = make_file("run.py", b"y");

        store_files(job_id, csv, script).await.unwrap();
        let job_dir = guard.path().join(job_id.to_string());
        assert!(job_dir.exists());

        cleanup_files(job_id).await.unwrap();
        assert!(!job_dir.exists());
    }

    #[tokio::test]
    async fn test_cleanup_files_removes_files_inside_directory() {
        let _guard = DataDirGuard::new();
        let job_id = Uuid::new_v4();
        let csv = make_file("data.csv", b"content");
        let script = make_file("run.py", b"code");

        let (csv_path, script_path) = store_files(job_id, csv, script).await.unwrap();
        assert!(csv_path.exists());
        assert!(script_path.exists());

        cleanup_files(job_id).await.unwrap();
        assert!(!csv_path.exists());
        assert!(!script_path.exists());
    }

    #[tokio::test]
    async fn test_cleanup_files_nonexistent_directory_returns_error() {
        let _guard = DataDirGuard::new();
        let job_id = Uuid::new_v4();
        // Don't create any files — directory doesn't exist

        let result = cleanup_files(job_id).await;
        assert!(result.is_err());
    }

    // --- store_files error path ---

    #[tokio::test]
    async fn test_store_files_unwritable_base_dir_returns_error() {
        let lock = ENV_LOCK.lock().unwrap();
        // Point DATA_DIR to a path that cannot be created.
        // On both Windows and Unix, a path rooted under a non-existent device/drive
        // or deeply nested under a file (not dir) will fail.
        let impossible_dir = std::env::temp_dir()
            .join(format!("foinc_test_{}", Uuid::new_v4()))
            .join("not_a_dir.txt");
        // Create a regular file at this path so it cannot be used as a directory.
        std::fs::create_dir_all(impossible_dir.parent().unwrap()).unwrap();
        std::fs::write(&impossible_dir, b"blocker").unwrap();

        // Set DATA_DIR to the file path — create_dir_all will fail because the
        // path component is a regular file, not a directory.
        std::env::set_var("DATA_DIR", impossible_dir.to_str().unwrap());

        let job_id = Uuid::new_v4();
        let csv = make_file("data.csv", b"x");
        let script = make_file("run.py", b"y");

        let result = store_files(job_id, csv, script).await;
        assert!(result.is_err(), "store_files should return an error when DATA_DIR is unwritable");

        // Clean up
        std::env::remove_var("DATA_DIR");
        let _ = std::fs::remove_dir_all(impossible_dir.parent().unwrap());
        drop(lock);
    }
}
