use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;

const MAX_CSV_SIZE: usize = 50 * 1024 * 1024; // 50 MB
const MAX_SCRIPT_SIZE: usize = 1 * 1024 * 1024; // 1 MB

/// A file extracted from the multipart upload, before validation.
pub struct UploadFile {
    pub filename: String,
    pub data: Vec<u8>,
}

/// Errors that can occur during upload validation.
#[derive(Debug)]
pub enum UploadError {
    InvalidExtension(String),
    FileTooLarge(String),
    MissingField(String),
}

impl IntoResponse for UploadError {
    fn into_response(self) -> Response {
        let message = match &self {
            UploadError::InvalidExtension(msg) => msg.clone(),
            UploadError::FileTooLarge(msg) => msg.clone(),
            UploadError::MissingField(msg) => msg.clone(),
        };
        (StatusCode::BAD_REQUEST, axum::Json(json!({ "error": message }))).into_response()
    }
}

/// Validate the uploaded CSV and script files against extension and size constraints.
///
/// Returns `Ok(())` if both files pass validation, or an `UploadError` describing
/// the first constraint violation found.
pub fn validate_upload(csv: &UploadFile, script: &UploadFile) -> Result<(), UploadError> {
    if !csv.filename.ends_with(".csv") {
        return Err(UploadError::InvalidExtension(
            "csv_file must have .csv extension".to_string(),
        ));
    }
    if !script.filename.ends_with(".py") {
        return Err(UploadError::InvalidExtension(
            "script_file must have .py extension".to_string(),
        ));
    }
    if csv.data.len() > MAX_CSV_SIZE {
        return Err(UploadError::FileTooLarge(
            "csv_file exceeds 50 MB limit".to_string(),
        ));
    }
    if script.data.len() > MAX_SCRIPT_SIZE {
        return Err(UploadError::FileTooLarge(
            "script_file exceeds 1 MB limit".to_string(),
        ));
    }
    Ok(())
}
