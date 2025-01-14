use actix_web::{get, web::Data, HttpResponse, Responder};

use crate::{
    api::{security::RequireApiKey, AppState, Repository},
    docker::get_container_execution_logs,
};

/// Hello world
#[utoipa::path(
    responses(
        (status = 200, description = "Said hi to the world", body = &str)
    )
)]
#[get("/health")]
async fn health() -> impl Responder {
    HttpResponse::Ok().json("Healthy")
}

/// Get system logs
#[utoipa::path(
    responses(
        (status = 200, description = "Fetched system logs", body = [Log])
    ),
    security(
        ("api_key" = [])
    )
)]
#[get("/system/logs", wrap = "RequireApiKey")]
async fn get_system_logs() -> impl Responder {
    let logs = get_container_execution_logs("prezel").await;
    HttpResponse::Ok().json(logs.collect::<Vec<_>>())
}
