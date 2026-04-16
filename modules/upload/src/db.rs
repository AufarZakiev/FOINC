use foinc_integrations::{Job, JobStatus};
use sqlx::PgPool;
use uuid::Uuid;

/// Insert a new job metadata row into the `jobs` Postgres table.
pub async fn insert_job(pool: &PgPool, job: &Job) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        INSERT INTO jobs (job_id, csv_filename, script_filename, csv_size_bytes, script_size_bytes, status, created_at)
        VALUES ($1, $2, $3, $4, $5, $6, $7)
        "#,
    )
    .bind(job.job_id)
    .bind(&job.csv_filename)
    .bind(&job.script_filename)
    .bind(job.csv_size_bytes)
    .bind(job.script_size_bytes)
    .bind(match &job.status {
        JobStatus::Uploaded => "uploaded",
    })
    .bind(job.created_at)
    .execute(pool)
    .await?;
    Ok(())
}

/// Fetch job metadata by ID from the `jobs` Postgres table.
///
/// Returns `None` if no job with the given ID exists.
pub async fn get_job(pool: &PgPool, id: Uuid) -> Result<Option<Job>, sqlx::Error> {
    let row = sqlx::query_as::<_, JobRow>(
        r#"
        SELECT job_id, csv_filename, script_filename, csv_size_bytes, script_size_bytes, status, created_at
        FROM jobs
        WHERE job_id = $1
        "#,
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(|r| r.into_job()))
}

/// Internal row type for mapping database results.
#[derive(sqlx::FromRow)]
struct JobRow {
    job_id: Uuid,
    csv_filename: String,
    script_filename: String,
    csv_size_bytes: i64,
    script_size_bytes: i64,
    status: String,
    created_at: chrono::DateTime<chrono::Utc>,
}

impl JobRow {
    fn into_job(self) -> Job {
        let status = match self.status.as_str() {
            "uploaded" => JobStatus::Uploaded,
            _ => JobStatus::Uploaded, // fallback for unknown statuses from downstream modules
        };
        Job {
            job_id: self.job_id,
            csv_filename: self.csv_filename,
            script_filename: self.script_filename,
            csv_size_bytes: self.csv_size_bytes,
            script_size_bytes: self.script_size_bytes,
            status,
            created_at: self.created_at,
        }
    }
}
