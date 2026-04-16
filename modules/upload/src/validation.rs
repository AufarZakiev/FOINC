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

#[cfg(test)]
mod tests {
    use super::*;
    use axum::response::IntoResponse;
    use http_body_util::BodyExt;

    fn make_file(name: &str, size: usize) -> UploadFile {
        UploadFile {
            filename: name.to_string(),
            data: vec![0u8; size],
        }
    }

    /// Helper: collect a response body into bytes.
    async fn body_bytes(response: axum::response::Response) -> Vec<u8> {
        response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .to_vec()
    }

    // --- Valid inputs ---

    #[test]
    fn test_validate_upload_valid_csv_and_py_returns_ok() {
        let csv = make_file("data.csv", 100);
        let script = make_file("run.py", 50);
        assert!(validate_upload(&csv, &script).is_ok());
    }

    // --- Extension checks ---

    #[test]
    fn test_validate_upload_csv_with_txt_extension_returns_invalid_extension() {
        let csv = make_file("data.txt", 100);
        let script = make_file("run.py", 50);
        let err = validate_upload(&csv, &script).unwrap_err();
        assert!(matches!(err, UploadError::InvalidExtension(_)));
    }

    #[test]
    fn test_validate_upload_csv_with_txt_extension_mentions_csv_file() {
        let csv = make_file("data.txt", 100);
        let script = make_file("run.py", 50);
        let err = validate_upload(&csv, &script).unwrap_err();
        match err {
            UploadError::InvalidExtension(msg) => assert!(msg.contains("csv_file")),
            other => panic!("Expected InvalidExtension, got {:?}", other),
        }
    }

    #[test]
    fn test_validate_upload_script_with_rb_extension_returns_invalid_extension() {
        let csv = make_file("data.csv", 100);
        let script = make_file("run.rb", 50);
        let err = validate_upload(&csv, &script).unwrap_err();
        assert!(matches!(err, UploadError::InvalidExtension(_)));
    }

    #[test]
    fn test_validate_upload_script_with_rb_extension_mentions_script_file() {
        let csv = make_file("data.csv", 100);
        let script = make_file("run.rb", 50);
        let err = validate_upload(&csv, &script).unwrap_err();
        match err {
            UploadError::InvalidExtension(msg) => assert!(msg.contains("script_file")),
            other => panic!("Expected InvalidExtension, got {:?}", other),
        }
    }

    #[test]
    fn test_validate_upload_csv_no_extension_returns_invalid_extension() {
        let csv = make_file("data", 100);
        let script = make_file("run.py", 50);
        assert!(matches!(
            validate_upload(&csv, &script).unwrap_err(),
            UploadError::InvalidExtension(_)
        ));
    }

    #[test]
    fn test_validate_upload_script_with_csv_extension_returns_invalid_extension() {
        let csv = make_file("data.csv", 100);
        let script = make_file("run.csv", 50);
        assert!(matches!(
            validate_upload(&csv, &script).unwrap_err(),
            UploadError::InvalidExtension(_)
        ));
    }

    // --- CSV size boundary ---

    #[test]
    fn test_validate_upload_csv_at_exact_50mb_returns_ok() {
        let csv = make_file("data.csv", 50 * 1024 * 1024);
        let script = make_file("run.py", 50);
        assert!(validate_upload(&csv, &script).is_ok());
    }

    #[test]
    fn test_validate_upload_csv_at_50mb_plus_one_returns_file_too_large() {
        let csv = make_file("data.csv", 50 * 1024 * 1024 + 1);
        let script = make_file("run.py", 50);
        let err = validate_upload(&csv, &script).unwrap_err();
        assert!(matches!(err, UploadError::FileTooLarge(_)));
    }

    #[test]
    fn test_validate_upload_csv_too_large_mentions_csv_file() {
        let csv = make_file("data.csv", 50 * 1024 * 1024 + 1);
        let script = make_file("run.py", 50);
        let err = validate_upload(&csv, &script).unwrap_err();
        match err {
            UploadError::FileTooLarge(msg) => assert!(msg.contains("csv_file")),
            other => panic!("Expected FileTooLarge, got {:?}", other),
        }
    }

    // --- Script size boundary ---

    #[test]
    fn test_validate_upload_script_at_exact_1mb_returns_ok() {
        let csv = make_file("data.csv", 100);
        let script = make_file("run.py", 1 * 1024 * 1024);
        assert!(validate_upload(&csv, &script).is_ok());
    }

    #[test]
    fn test_validate_upload_script_at_1mb_plus_one_returns_file_too_large() {
        let csv = make_file("data.csv", 100);
        let script = make_file("run.py", 1 * 1024 * 1024 + 1);
        let err = validate_upload(&csv, &script).unwrap_err();
        assert!(matches!(err, UploadError::FileTooLarge(_)));
    }

    #[test]
    fn test_validate_upload_script_too_large_mentions_script_file() {
        let csv = make_file("data.csv", 100);
        let script = make_file("run.py", 1 * 1024 * 1024 + 1);
        let err = validate_upload(&csv, &script).unwrap_err();
        match err {
            UploadError::FileTooLarge(msg) => assert!(msg.contains("script_file")),
            other => panic!("Expected FileTooLarge, got {:?}", other),
        }
    }

    // --- Extension checked before size ---

    #[test]
    fn test_validate_upload_checks_csv_extension_before_script_extension() {
        // Both have wrong extensions; csv is checked first
        let csv = make_file("data.txt", 100);
        let script = make_file("run.rb", 50);
        let err = validate_upload(&csv, &script).unwrap_err();
        match err {
            UploadError::InvalidExtension(msg) => assert!(msg.contains("csv_file")),
            other => panic!("Expected InvalidExtension for csv_file, got {:?}", other),
        }
    }

    #[test]
    fn test_validate_upload_checks_extension_before_size() {
        // Wrong extension AND too large — extension error should come first
        let csv = make_file("data.txt", 50 * 1024 * 1024 + 1);
        let script = make_file("run.py", 50);
        let err = validate_upload(&csv, &script).unwrap_err();
        assert!(matches!(err, UploadError::InvalidExtension(_)));
    }

    // --- UploadError IntoResponse tests ---

    #[tokio::test]
    async fn test_upload_error_invalid_extension_returns_400() {
        let err = UploadError::InvalidExtension("bad ext".to_string());
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_upload_error_invalid_extension_returns_json_error_body() {
        let err = UploadError::InvalidExtension("csv_file must have .csv extension".to_string());
        let response = err.into_response();
        let bytes = body_bytes(response).await;
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            body,
            serde_json::json!({ "error": "csv_file must have .csv extension" })
        );
    }

    #[tokio::test]
    async fn test_upload_error_file_too_large_returns_400() {
        let err = UploadError::FileTooLarge("csv_file exceeds 50 MB limit".to_string());
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_upload_error_missing_field_returns_400() {
        let err = UploadError::MissingField("csv_file field is required".to_string());
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
}
