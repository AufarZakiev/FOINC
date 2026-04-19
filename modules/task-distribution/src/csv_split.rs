use std::io;
use std::path::Path;

/// Read a CSV file from disk and return its data rows.
///
/// The file is split on `\n`, each line is trimmed of surrounding whitespace
/// (including trailing `\r` from CRLF line endings), empty lines are
/// dropped, and then the first remaining line is dropped as the header.
///
/// Returns `Ok(vec![])` when the file is empty or contains only a header
/// or only whitespace — the caller is responsible for deciding whether an
/// empty task list is valid.
pub async fn split_csv(path: &Path) -> Result<Vec<String>, io::Error> {
    let contents = tokio::fs::read_to_string(path).await?;
    Ok(split_csv_text(&contents))
}

/// Pure-string form of [`split_csv`] for easy unit testing.
pub fn split_csv_text(contents: &str) -> Vec<String> {
    let mut non_empty = contents
        .split('\n')
        .map(|line| line.trim().to_string())
        .filter(|line| !line.is_empty());

    // Drop the first non-empty line (header).
    let _header = non_empty.next();
    non_empty.collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drops_header_and_returns_data_rows() {
        let csv = "col1,col2\n1,2\n3,4\n";
        assert_eq!(split_csv_text(csv), vec!["1,2".to_string(), "3,4".to_string()]);
    }

    #[test]
    fn trims_whitespace_and_crlf() {
        let csv = "col\r\n a \r\nb\r\n";
        assert_eq!(split_csv_text(csv), vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn empty_lines_are_dropped_before_header_detection() {
        let csv = "\n\nheader\n\nrow1\n\nrow2\n";
        assert_eq!(
            split_csv_text(csv),
            vec!["row1".to_string(), "row2".to_string()]
        );
    }

    #[test]
    fn returns_empty_when_only_header() {
        assert_eq!(split_csv_text("header\n"), Vec::<String>::new());
    }

    #[test]
    fn returns_empty_when_file_blank() {
        assert_eq!(split_csv_text(""), Vec::<String>::new());
    }
}
