use api::server::run_api_server;
use conf::Conf;
use db::Db;
use deployments::manager::Manager;
use github::Github;
use proxy::run_proxy;
use tls::CertificateStore;
use traces::init_tracing_subscriber;
use tracing::info;

mod api;
mod conf;
mod container;
mod db;
mod deployments;
mod docker;
mod docker_bridge;
mod env;
mod github;
mod hooks;
mod label;
mod listener;
mod logging;
mod nixpacks;
mod paths;
mod provider;
mod proxy;
mod sqlite_db;
mod tls;
mod tokens;
mod traces;
mod utils;

pub(crate) const DOCKER_PORT: u16 = 5046;

#[tokio::main]
async fn main() {
    let _guard = init_tracing_subscriber();
    info!("prezel is starting...");

    let conf = Conf::read();
    let cloned_conf = conf.clone();

    let db = Db::setup().await;
    let github = Github::new().await;

    provider::setup_ip_address().await.unwrap();

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

    run_api_server(manager, db, github, &conf.api_hostname(), conf.secret)
        .await
        .unwrap();
}
