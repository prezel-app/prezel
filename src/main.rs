use api::server::run_api_server;
use conf::Conf;
use db::Db;
use deployments::manager::Manager;
use github::Github;
use proxy::run_proxy;
use tls::CertificateStore;
use traces::init_tracing_subscriber;
use tracing::info;

mod alphabet;
mod api;
mod conf;
mod container;
mod db;
mod deployment_hooks;
mod deployments;
mod docker;
mod docker_bridge;
mod env;
mod github;
mod label;
mod listener;
mod logging;
mod paths;
mod proxy;
mod sqlite_db;
mod time;
mod tls;
mod traces;

pub(crate) const DOCKER_PORT: u16 = 5046;

// struct DeploymentFilter;

// impl Filter<Registry> for DeploymentFilter {
//     fn enabled(
//         &self,
//         meta: &tracing::Metadata<'_>,
//         _ctx: &tracing_subscriber::layer::Context<'_, Registry>,
//     ) -> bool {
//         meta.fields().field("deployment").is_some() // TODO: rename this field to prezel?
//     }
// }

#[tokio::main]
async fn main() {
    let _guard = init_tracing_subscriber();
    info!("prezel is starting...");

    // old tracing conf
    /////////////////////////////////////////////////////////////////
    // let stdout_layer = tracing_subscriber::fmt::layer()
    //     .pretty()
    //     .with_writer(std::io::stdout)
    //     .with_filter(EnvFilter::new("info")); // TODO: read from env
    // tracing_subscriber::registry()
    //     .with(stdout_layer)
    //     .init();
    /////////////////////////////////////////////////////////////////

    let conf = Conf::read();
    let cloned_conf = conf.clone();

    let db = Db::setup().await;
    let github = Github::new().await;

    let certificates = CertificateStore::load(&conf).await;
    let manager = Manager::new(
        conf.hostname.clone(),
        github.clone(),
        db.clone(),
        certificates.clone(),
    );
    let cloned_manager = manager.clone();

    tokio::task::spawn_blocking(|| run_proxy(cloned_manager, cloned_conf, certificates));

    manager.full_sync_with_github().await;

    let api_hostname = format!("api.{}", &conf.hostname);
    run_api_server(manager, db, github, &api_hostname, conf.coordinator)
        .await
        .unwrap();
}
