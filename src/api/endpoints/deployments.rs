use actix_web::{
    delete, get, post,
    web::{Data, Json, Path},
    HttpResponse, Responder,
};

use crate::{
    api::{
        bearer::{AdminRole, AnyRole},
        utils::clone_deployment,
        AppState,
    },
    logging::{read_request_event_logs, Log},
};

// TODO: this should take the id from the PATH, should not be POST I guess
/// Re-deploy based on an existing deployment
#[utoipa::path(
    request_body = String,
    responses(
        (status = 200, description = "Deployment redeployed successfully"),
    ),
    security(
        ("bearerAuth" = [])
    )
)]
#[post("/api/deployments/redeploy")]
#[tracing::instrument]
async fn redeploy(
    auth: AdminRole,
    deployment: Json<String>,
    state: Data<AppState>,
) -> impl Responder {
    clone_deployment(&state.db, &deployment.0.into()).await;
    state.manager.sync_with_db().await;
    HttpResponse::Ok()
}

/// Delete deployment
#[utoipa::path(
    responses(
        (status = 200, description = "Deployment deleted successfully"),
    ),
    security(
        ("bearerAuth" = [])
    )
)]
#[delete("/api/deployments/{id}")]
#[tracing::instrument]
async fn delete_deployment(
    auth: AdminRole,
    state: Data<AppState>,
    id: Path<String>,
) -> impl Responder {
    state
        .db
        .delete_deployment(&id.into_inner().into())
        .await
        .unwrap();
    state.manager.sync_with_db().await;
    HttpResponse::Ok()
}

/// Sync deployments with github
#[utoipa::path(
    responses(
        (status = 200, description = "Sync triggered successfully"),
    ),
    security(
        ("bearerAuth" = [])
    )
)]
#[post("/api/deployments/sync")]
#[tracing::instrument]
async fn sync(auth: AdminRole, state: Data<AppState>) -> impl Responder {
    state.manager.full_sync_with_github().await;
    HttpResponse::Ok()
}

/// Get deployment execution logs
#[utoipa::path(
    responses(
        (status = 200, description = "Fetched deployment execution logs", body = [Log]),
        (status = 404, description = "Deployment not found", body = String),
        (status = 500, description = "Internal error when fetching logs", body = String)
    ),
    security(
        ("bearerAuth" = [])
    )
)]
#[get("/api/deployments/{id}/logs")]
#[tracing::instrument]
async fn get_deployment_logs(
    auth: AnyRole,
    state: Data<AppState>,
    id: Path<String>,
) -> impl Responder {
    let id = id.into_inner().into();
    let app_container = match state.manager.get_deployment(&id).await {
        Some(deployment) => deployment.app_container,
        None => return HttpResponse::NotFound().json("not found"),
    };

    let container_logs = app_container
        .get_logs()
        .await
        .map(|log| Log::from_docker(log, id.clone()));

    match read_request_event_logs() {
        Ok(logs) => {
            let mut logs = logs
                .filter(|log| &log.deployment == id.as_str())
                .chain(container_logs)
                .collect::<Vec<_>>();
            logs.sort_by_key(|log| -log.time); // from latest to oldest
            HttpResponse::Ok().json(logs)
        }
        Err(error) => HttpResponse::InternalServerError().json(error.to_string()), // need a ErrorResponse variant for this
    }
}

/// Get deployment build logs
#[utoipa::path(
    responses(
        (status = 200, description = "Fetched deployment build logs", body = [Log]),
        // (status = 404, description = "Deployment not found", body = String),
        // (status = 500, description = "Internal error when fetching logs", body = String) // TODO: re-enable errors
    ),
    security(
        ("bearerAuth" = [])
    )
)]
#[get("/api/deployments/{id}/build")]
#[tracing::instrument]
async fn get_deployment_build_logs(
    auth: AnyRole,
    state: Data<AppState>,
    id: Path<String>,
) -> impl Responder {
    let id = id.into_inner().into();
    let logs: Vec<Log> = state
        .db
        .get_deployment_build_logs(&id)
        .await
        .unwrap()
        .into_iter()
        .map(|log| log.into())
        .collect();
    HttpResponse::Ok().json(logs)
}
