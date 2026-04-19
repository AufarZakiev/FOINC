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

/// Delete the job row with the given ID from the `jobs` Postgres table.
///
/// Returns `Ok(true)` if a row was deleted, `Ok(false)` if no row matched
/// (i.e. the job does not exist). Callers use the boolean to decide whether
/// to touch the filesystem: on `false`, skip IO and return `404`.
pub async fn delete_job(pool: &PgPool, id: Uuid) -> Result<bool, sqlx::Error> {
    let result = sqlx::query(
        r#"
        DELETE FROM jobs
        WHERE job_id = $1
        "#,
    )
    .bind(id)
    .execute(pool)
    .await?;

    Ok(result.rows_affected() > 0)
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

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use sqlx::postgres::PgPoolOptions;

    /// Connect to the test Postgres instance configured via `DATABASE_URL`.
    ///
    /// Returns `None` when the env var is unset so local runs without the DB
    /// stack simply skip these integration-flavoured tests instead of
    /// failing. CI and any developer running docker-compose will have the
    /// var set and exercise the real DB.
    async fn pool_or_skip() -> Option<PgPool> {
        let url = std::env::var("DATABASE_URL").ok()?;
        let pool = PgPoolOptions::new()
            .max_connections(2)
            .connect(&url)
            .await
            .expect("DATABASE_URL set but connection failed");
        Some(pool)
    }

    fn sample_job(job_id: Uuid) -> Job {
        Job {
            job_id,
            csv_filename: "data.csv".to_string(),
            script_filename: "run.py".to_string(),
            csv_size_bytes: 10,
            script_size_bytes: 5,
            status: JobStatus::Uploaded,
            created_at: Utc::now(),
        }
    }

    #[tokio::test]
    async fn test_delete_job_returns_true_when_row_exists() {
        let Some(pool) = pool_or_skip().await else {
            return;
        };

        let job_id = Uuid::new_v4();
        insert_job(&pool, &sample_job(job_id)).await.unwrap();

        let deleted = delete_job(&pool, job_id).await.unwrap();
        assert!(deleted, "delete_job should return true for an existing row");

        // And the row must really be gone.
        let after = get_job(&pool, job_id).await.unwrap();
        assert!(after.is_none(), "row should be gone after delete_job");
    }

    #[tokio::test]
    async fn test_delete_job_returns_false_when_row_missing() {
        let Some(pool) = pool_or_skip().await else {
            return;
        };

        let job_id = Uuid::new_v4();
        // Do NOT insert. Deleting a non-existent row should succeed with
        // `Ok(false)` so callers can distinguish missing from error.
        let deleted = delete_job(&pool, job_id).await.unwrap();
        assert!(
            !deleted,
            "delete_job should return false when no row matches"
        );
    }
}
