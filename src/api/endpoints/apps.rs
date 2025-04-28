use actix_web::{
    delete, get, patch, post,
    web::{Data, Json, Path},
    HttpResponse, Responder,
};
use futures::future::join_all;

use crate::{
    api::{
        bearer::{AdminRole, AnyRole},
        utils::{
            get_all_deployments, get_prod_deployment, get_prod_deployment_id, is_app_name_valid,
        },
        AppState, ErrorResponse, FullProjectInfo, ProjectInfo,
    },
    db::{nano_id::IntoOptString, EnvVar, InsertProject, UpdateProject},
};

/// Get projects
#[utoipa::path(
    responses(
        (status = 200, description = "Projets returned successfully", body = [ProjectInfo])
    ),
    security(
        ("bearerAuth" = [])
    )
)]
#[get("/api/apps")]
#[tracing::instrument]
async fn get_projects(auth: AnyRole, state: Data<AppState>) -> impl Responder {
    let projects = state.db.get_projects().await.unwrap();
    let db_access = auth.0.role.get_db_access();
    let projects_with_deployments = projects.into_iter().map(|project| {
        let state = state.clone();
        async move {
            let prod_deployment = get_prod_deployment(&state, &project.id, db_access).await;
            let prod_deployment_id = get_prod_deployment_id(&state.db, &project).await;
            ProjectInfo {
                name: project.name.clone(),
                id: project.id.to_string(),
                repo: project.repo_id,
                created: project.created,
                custom_domains: project.custom_domains,
                prod_deployment_id: prod_deployment_id.into_opt_string(),
                prod_deployment,
            }
        }
    });

    HttpResponse::Ok().json(join_all(projects_with_deployments).await)
}

/// Get project by name
#[utoipa::path(
    responses(
        (status = 200, description = "Projet returned successfully", body = FullProjectInfo),
        (status = 404, description = "Project not found", body = ErrorResponse)
    ),
    security(
        ("bearerAuth" = [])
    )
)]
#[get("/api/apps/{name}")]
#[tracing::instrument]
async fn get_project(auth: AnyRole, state: Data<AppState>, name: Path<String>) -> impl Responder {
    let name = name.into_inner();
    let project = state.db.get_project_by_name(&name).await.unwrap();
    let db_access = auth.0.role.get_db_access();
    match project {
        Some(project) => {
            let prod_deployment_id = get_prod_deployment_id(&state.db, &project).await;
            let prod_deployment = get_prod_deployment(&state, &project.id, db_access).await;
            let deployments = get_all_deployments(&state, &project.id, db_access).await;
            HttpResponse::Ok().json(FullProjectInfo {
                name: project.name,
                id: project.id.into(),
                repo: project.repo_id,
                created: project.created,
                custom_domains: project.custom_domains,
                prod_deployment_id: prod_deployment_id.into_opt_string(),
                prod_deployment,
                deployments,
            })
        }
        None => HttpResponse::NotFound().json(ErrorResponse::NotFound(format!("name = {name}"))),
    }
}

/// Create project
#[utoipa::path(
    request_body = InsertProject,
    responses(
        (status = 201, description = "Project created successfully"),
        (status = 400, description = "App name is not valid"),
    ),
    security(
        ("bearerAuth" = [])
    )
)]
#[post("/api/apps")] // TODO: return project when successfully inserted
#[tracing::instrument]
async fn create_project(
    _auth: AdminRole,
    project: Json<InsertProject>,
    state: Data<AppState>,
) -> impl Responder {
    if is_app_name_valid(&project.name) {
        state.db.insert_project(project.0).await.unwrap();
        state.manager.full_sync_with_github().await;
        HttpResponse::Ok()
    } else {
        HttpResponse::BadRequest()
    }
}

/// Update project
#[utoipa::path(
    request_body = UpdateProject,
    responses(
        (status = 200, description = "Project updated successfully"),
        (status = 400, description = "App name is not valid"),
        // (status = 409, description = "Todo with id already exists", body = ErrorResponse, example = json!(ErrorResponse::Conflict(String::from("id = 1"))))
    ),
    security(
        ("bearerAuth" = [])
    )
)]
#[patch("/api/apps/{id}")]
#[tracing::instrument]
async fn update_project(
    auth: AdminRole,
    project: Json<UpdateProject>,
    state: Data<AppState>,
    id: Path<String>,
) -> impl Responder {
    let valid_name = project
        .0
        .name
        .as_ref()
        .is_none_or(|name| is_app_name_valid(name));
    if valid_name {
        let id = id.into_inner().into();
        state.db.update_project(&id, project.0).await.unwrap();
        state.manager.sync_with_db().await; // TODO: review if its fine not doing a full sync with github here
        HttpResponse::Ok()
    } else {
        HttpResponse::BadRequest()
    }
}

/// Delete project
#[utoipa::path(
    responses(
        (status = 200, description = "Project deleted successfully"),
    ),
    security(
        ("bearerAuth" = [])
    )
)]
#[delete("/api/apps/{id}")]
#[tracing::instrument]
async fn delete_project(
    auth: AdminRole,
    state: Data<AppState>,
    id: Path<String>,
) -> impl Responder {
    state
        .db
        .delete_project(&id.into_inner().into())
        .await
        .unwrap();
    state.manager.sync_with_db().await;
    HttpResponse::Ok()
}

/// Get env
#[utoipa::path(
    responses(
        (status = 200, description = "Env returned successfully", body = [EditedEnvVar]),
        (status = 404, description = "Project not found", body = ErrorResponse)
    ),
    security(
        ("bearerAuth" = [])
    )
)]
#[get("/api/apps/{id}/env")]
#[tracing::instrument]
async fn get_env(auth: AdminRole, state: Data<AppState>, id: Path<String>) -> impl Responder {
    let id = id.into_inner().into();
    match state.db.get_project(&id).await.unwrap() {
        Some(project) => HttpResponse::Ok().json(project.env),
        None => HttpResponse::NotFound().json(ErrorResponse::NotFound(format!("id = {id}"))),
    }
}

/// Upsert env
#[utoipa::path(
    request_body = EnvVar,
    responses(
        (status = 200, description = "Env upserted successfully"),
    ),
    security(
        ("bearerAuth" = [])
    )
)]
#[patch("/api/apps/{id}/env")]
#[tracing::instrument]
async fn upsert_env(
    auth: AdminRole,
    env: Json<EnvVar>,
    state: Data<AppState>,
    id: Path<String>,
) -> impl Responder {
    let id = id.into_inner().into();
    state
        .db
        .upsert_env(&id, &env.0.name, &env.0.value)
        .await
        .unwrap();
    // state.manager.sync_with_db().await; // TODO: review if its fine not calling sync here
    HttpResponse::Ok()
}

/// Delete env
#[utoipa::path(
    responses(
        (status = 200, description = "Env deleted successfully"),
    ),
    security(
        ("bearerAuth" = [])
    )
)]
#[delete("/api/apps/{id}/env/{name}")]
#[tracing::instrument]
async fn delete_env(
    auth: AdminRole,
    state: Data<AppState>,
    path: Path<(String, String)>,
) -> impl Responder {
    state
        .db
        .delete_env(&(path.0.clone().into()), &path.1)
        .await
        .unwrap();
    // state.manager.sync_with_db().await; // TODO: review if its fine not calling sync here
    HttpResponse::Ok()
}
