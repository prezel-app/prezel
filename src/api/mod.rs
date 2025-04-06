use actix_web::web::{Data, ServiceConfig};
use endpoints::{apps, deployments, system, version};
use octocrab::models::Repository as CrabRepository;
use serde::Serialize;
use utoipa::{OpenApi, ToSchema};

use crate::{
    db::{
        BuildResult, Db, DeploymentWithProject, EditedEnvVar, EnvVar, InsertProject, UpdateProject,
    },
    deployments::{deployment::Deployment, manager::Manager},
    docker::get_image,
    github::Github,
    logging::{Level, Log},
    sqlite_db::DbAccess,
    utils::PlusHttps,
};

mod bearer;
mod endpoints;
pub(crate) mod server;
mod utils;

pub(crate) const API_PORT: u16 = 5045;

// TODO: move this to routes.rs so I don't forget updating them
#[derive(OpenApi)]
#[openapi(
    paths(
        version::get_version,
        version::update_version,
        system::get_logs,
        apps::get_projects,
        apps::get_project,
        apps::create_project,
        apps::update_project,
        apps::delete_project,
        apps::get_env,
        apps::upsert_env,
        apps::delete_env,
        deployments::redeploy,
        deployments::delete_deployment,
        deployments::sync,
        deployments::get_deployment_logs,
        deployments::get_deployment_build_logs
    ),
    components(schemas(ProjectInfo, FullProjectInfo, ErrorResponse, UpdateProject, Repository, ApiDeployment, Log, Level, Status, InsertProject, LibsqlDb, EnvVar, EditedEnvVar, Certificate)),
    tags(
        (name = "prezel", description = "Prezel management endpoints.")
    ),
)]
struct ApiDoc;

fn configure_service(store: Data<AppState>) -> impl FnOnce(&mut ServiceConfig) {
    |config: &mut ServiceConfig| {
        config
            .app_data(store)
            .service(version::get_version)
            .service(version::update_version)
            .service(system::get_logs)
            .service(system::get_certs)
            .service(apps::get_projects)
            .service(apps::get_project)
            .service(apps::create_project)
            .service(apps::update_project)
            .service(apps::delete_project)
            .service(apps::get_env)
            .service(apps::upsert_env)
            .service(apps::delete_env)
            .service(deployments::redeploy)
            .service(deployments::delete_deployment)
            .service(deployments::sync)
            .service(deployments::get_deployment_logs)
            .service(deployments::get_deployment_build_logs);
        // If I add anything here also need to add it in api/mod.rs
    }
}

// TODO: there is some duplication here, because manager holds db and github as well
#[derive(Clone, Debug)]
pub(crate) struct AppState {
    pub(crate) db: Db,
    pub(crate) manager: Manager,
    pub(crate) github: Github,
    pub(crate) secret: Vec<u8>,
}

#[derive(Serialize, ToSchema)]
enum ErrorResponse {
    /// When Todo is not found by search term.
    NotFound(String),
    /// When there is a conflict storing a new todo.
    Conflict(String),
    /// When todo endpoint was called without correct credentials
    Unauthorized(String),
}

#[derive(Debug, PartialEq, Clone, Copy, ToSchema, Serialize)]
pub(crate) enum Status {
    Built,
    StandBy,
    Queued,
    Building,
    Ready,
    Failed,
}

impl ToString for Status {
    fn to_string(&self) -> String {
        let string = match self {
            Self::Built => "built",
            Self::Queued => "queued",
            Self::Building => "building",
            Self::StandBy => "stand by",
            Self::Ready => "ready",
            Self::Failed => "failed",
        };
        string.to_owned()
    }
}

#[derive(Serialize, ToSchema)]
struct LibsqlDb {
    url: String,
    token: String,
}

#[derive(Serialize, ToSchema)]
#[schema(title = "Deployment")]
struct ApiDeployment {
    id: String,
    url_id: String,
    // project: Project, // TODO: review why I needed this
    sha: String,
    gitref: String,
    // port: u16,
    url: Option<String>,
    target_url: Option<String>,
    custom_urls: Vec<String>,
    libsql_db: Option<LibsqlDb>,
    status: Status,
    app_container: Option<String>,
    image_size: Option<i64>,
    // execution_logs: Vec<DockerLog>,
    created: i64,
    build_started: Option<i64>,
    build_finished: Option<i64>,
}

// TODO: move this somewhere else
impl ApiDeployment {
    // TODO: make info an option so deployments can show up in the API before the manager reads them
    #[tracing::instrument]
    async fn from(
        deployment: Option<&Deployment>,
        db_deployment: &DeploymentWithProject,
        is_prod: bool,
        box_domain: &str,
        manager: &Manager,
        access: DbAccess,
    ) -> Self {
        let (status, url, prod_url, custom_urls, app_container, image_size, libsql_db) =
            if let Some(deployment) = deployment {
                let container_status = deployment.app_container.status.read().await.clone();
                let status = container_status.to_status();
                let image_name = container_status.get_image_name();
                let image_size = if let Some(name) = image_name {
                    get_image(name).await.and_then(|image| image.size)
                } else {
                    None
                };

                let url = Some(db_deployment.get_app_base_url(box_domain));
                let prod_url = is_prod.then_some(db_deployment.get_prod_base_url(box_domain));
                let custom_urls = if is_prod {
                    db_deployment
                        .project
                        .custom_domains
                        .iter()
                        .map(|domain| domain.plus_https())
                        .collect()
                } else {
                    vec![]
                };

                // FIXME: maybe only expose the container name in the api if the container really exists
                let app_container = deployment.app_container.get_container_name().await;

                let db_url = db_deployment.get_libsql_url(box_domain);
                let libsql_db = if is_prod {
                    let prod_db = manager.get_prod_db(&deployment.project).await;
                    prod_db.map(|setup| LibsqlDb {
                        url: db_url,
                        token: setup.auth.generate_expiring_token(access),
                    })
                } else {
                    let branch_db = container_status.get_db_setup();
                    branch_db.map(|setup| LibsqlDb {
                        url: db_url,
                        token: setup.auth.generate_expiring_token(access),
                    })
                };

                (
                    status,
                    url,
                    prod_url,
                    custom_urls,
                    app_container,
                    image_size,
                    libsql_db,
                )
            } else {
                let status = match db_deployment.result {
                    Some(BuildResult::Failed) => Status::Failed,
                    Some(BuildResult::Built) => Status::Built,
                    None => Status::Queued,
                };
                (status, None, None, vec![], None, None, None)
            };

        // TODO: I should have a nested struct for the container related
        // info so it can be an option as a whole
        Self {
            id: db_deployment.id.to_string(),
            url_id: db_deployment.url_id.clone(),
            // project: value.deployment.project.clone(),// TODO: review why I needed this
            sha: db_deployment.sha.clone(),
            gitref: db_deployment.branch.clone(),
            url, // TODO: add method to get the http version from the same object !!!
            target_url: prod_url,
            custom_urls,
            libsql_db,
            status,
            app_container,
            image_size,
            created: db_deployment.created,
            build_started: db_deployment.build_started,
            build_finished: db_deployment.build_finished,
        }
    }
}

#[derive(Serialize, ToSchema)]
struct Repository {
    id: String,
    name: String,
    owner: Option<String>,
    default_branch: Option<String>,
    pushed_at: Option<i64>,
}

impl From<CrabRepository> for Repository {
    fn from(value: CrabRepository) -> Self {
        Self {
            id: value.id.to_string(),
            name: value.name,
            owner: value.owner.map(|owner| owner.login),
            default_branch: value.default_branch,
            pushed_at: value.pushed_at.map(|at| at.timestamp_millis()),
        }
    }
}

#[derive(Serialize, ToSchema)]
struct ProjectInfo {
    name: String,
    id: String,
    repo: i64,
    created: i64,
    custom_domains: Vec<String>,
    prod_deployment_id: Option<String>,
    prod_deployment: Option<ApiDeployment>,
}

#[derive(Serialize, ToSchema)]
struct FullProjectInfo {
    name: String,
    id: String,
    repo: i64,
    created: i64,
    custom_domains: Vec<String>,
    prod_deployment_id: Option<String>,
    prod_deployment: Option<ApiDeployment>,
    /// All project deployments sorted by created datetime descending
    deployments: Vec<ApiDeployment>,
}

#[derive(Serialize, ToSchema)]
struct Certificate {
    domain: String,
    expiring: i64,
    issuer_org: String,
    issuer_name: String,
    issuer_country: String,
}
