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
