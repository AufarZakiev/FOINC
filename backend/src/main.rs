use axum::routing::{delete, get, post};
use axum::Router;
use sqlx::postgres::PgPoolOptions;
use tower_http::cors::{Any, CorsLayer};

#[tokio::main]
async fn main() {
    let database_url =
        std::env::var("DATABASE_URL").expect("DATABASE_URL environment variable must be set");

    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect(&database_url)
        .await
        .expect("Failed to connect to Postgres");

    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    let app = Router::new()
        .route("/upload", post(foinc_upload::upload_handler))
        .route("/jobs/{id}", get(foinc_upload::get_job_handler))
        .route("/jobs/{id}", delete(foinc_upload::delete_job_handler))
        .route(
            "/jobs/{id}/start",
            post(foinc_task_distribution::start_job_handler),
        )
        .route(
            "/tasks/next",
            post(foinc_task_distribution::next_task_handler),
        )
        .route(
            "/tasks/{id}/submit",
            post(foinc_task_distribution::submit_task_handler),
        )
        .route(
            "/tasks/stats",
            get(foinc_task_distribution::task_stats_handler),
        )
        .route(
            "/jobs/{id}/result",
            get(foinc_result_aggregation::get_result_handler),
        )
        .layer(cors)
        .with_state(pool);

    let bind_addr = std::env::var("BIND_ADDR").unwrap_or_else(|_| "0.0.0.0:3000".to_string());
    let listener = tokio::net::TcpListener::bind(&bind_addr)
        .await
        .expect("Failed to bind to address");

    println!("FOINC backend listening on {}", bind_addr);
    axum::serve(listener, app).await.expect("Server error");
}
