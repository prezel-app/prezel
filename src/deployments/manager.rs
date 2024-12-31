use std::{backtrace::Backtrace, sync::Arc, time::Duration};

use futures::{stream, StreamExt};
use tokio::sync::{RwLock, RwLockReadGuard};

use crate::{
    container::Container, db::Db, github::Github, label::Label, sqlite_db::SqliteDbSetup,
    time::now, tls::CertificateStore,
};

use super::{
    deployment::Deployment,
    map::DeploymentMap,
    worker::{Worker, WorkerHandle},
    workers::{build::BuildWorker, docker::DockerWorker, github::GithubWorker},
};

#[derive(Clone, Debug)]
pub(crate) struct Manager {
    pub(crate) box_domain: String,
    deployments: Arc<InstrumentedRwLock<DeploymentMap>>,
    build_worker: Arc<WorkerHandle>,
    github_worker: Arc<WorkerHandle>,
    docker_worker: Arc<WorkerHandle>,
    db: Db,
    github: Github,
}

// workers:
// - github worker
// - db worker
// - build worker

impl Manager {
    #[tracing::instrument]
    pub(crate) fn new(
        box_domain: String,
        github: Github,
        db: Db,
        certificates: CertificateStore,
    ) -> Self {
        let deployments: Arc<_> = InstrumentedRwLock::new(DeploymentMap::new(certificates)).into();

        // TODO: add docker or clean worker and trigger it at the end of the deployment worker flow

        let github_clone = github.clone();
        let db_clone = db.clone();
        let deployments_clone = deployments.clone();
        let build_worker: Arc<_> = BuildWorker::start(move |build_queue| BuildWorker {
            map: deployments_clone,
            db: db_clone,
            github: github_clone,
            build_queue,
        })
        .into();

        let github_worker = GithubWorker::start(|_| GithubWorker {
            github: github.clone(),
            db: db.clone(),
        })
        .into();

        let deployments_clone = deployments.clone();
        let docker_worker = DockerWorker::start(|_| DockerWorker {
            map: deployments_clone,
        })
        .into();

        let manager = Self {
            deployments,
            box_domain,
            build_worker,
            github_worker,
            docker_worker,
            db,
            github,
        };

        // TODO: reset the timer every time full_sync_with_github is executed triggered by something else
        let cloned_manager = manager.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(60 * 5)); // Every 5 minutes
            loop {
                interval.tick().await;
                cloned_manager.full_sync_with_github().await;
            }
        });

        manager
    }

    #[tracing::instrument]
    pub(crate) async fn get_container_by_hostname(&self, hostname: &str) -> Option<Arc<Container>> {
        let container = self
            .deployments
            .read()
            .await
            .get_custom_domain(hostname)
            .map(|deployment| deployment.app_container.clone());
        if let Some(container) = container {
            Some(container)
        } else {
            let labels = Label::strip_from_domain(hostname, &self.box_domain).ok()?;
            let containers =
                stream::iter(labels).filter_map(|label| self.get_container_by_label(label));
            Box::pin(containers).next().await
        }
    }

    #[tracing::instrument]
    async fn get_container_by_label(&self, label: Label) -> Option<Arc<Container>> {
        let map = self.deployments.read().await;
        match &label {
            Label::Prod { project } => {
                let deployment = map.get_prod(project)?;
                Some(deployment.app_container.clone())
            }
            Label::Deployment {
                project,
                deployment,
            } => {
                let deployment = map.get_deployment(project, deployment)?;
                Some(deployment.app_container.clone())
            }
            Label::BranchDb {
                project,
                deployment,
            } => {
                let deployment = map.get_deployment(project, deployment)?;
                let status = &deployment.app_container.status;
                status
                    .read()
                    .await
                    .get_db_setup()
                    .map(|setup| setup.container.clone())
            }
            Label::ProdDb { project } => map
                .get_prod_db_by_name(project)
                .map(|setup| setup.container.clone()),
        }
    }

    #[tracing::instrument]
    pub(crate) async fn get_deployment(&self, id: i64) -> Option<RwLockReadGuard<Deployment>> {
        let map = self.deployments.read().await;
        RwLockReadGuard::try_map(map, |map| {
            let (_, deployment) = map
                .deployments
                .iter()
                .find(|(_, deployment)| deployment.id == id)?;
            Some(deployment)
        })
        .ok()
    }

    #[tracing::instrument]
    pub(crate) async fn get_prod_deployment(
        &self,
        project: i64,
    ) -> Option<RwLockReadGuard<Deployment>> {
        let map = self.deployments.read().await;
        RwLockReadGuard::try_map(map, |map| {
            let prod_id = map.prod.get(&project)?;
            map.deployments.get(&(project, prod_id.to_owned()))
        })
        .ok()
    }

    #[tracing::instrument]
    pub(crate) async fn get_prod_db(&self, project: i64) -> Option<SqliteDbSetup> {
        self.deployments.read().await.get_prod_db(project)
    }

    #[tracing::instrument]
    pub(crate) async fn get_prod_url_id(&self, project: i64) -> Option<String> {
        let map = self.deployments.read().await;
        Some(map.prod.get(&project)?.to_owned())
    }

    #[tracing::instrument]
    pub(crate) async fn sync_with_db(&self) {
        self.deployments
            .write()
            .await
            .read_db_and_build_updates(&self.build_worker, &self.github, &self.db)
            .await;
        self.build_worker.trigger();
        self.docker_worker.trigger();
    }

    /// this triggers all the sync workflows downstream
    #[tracing::instrument]
    pub(crate) async fn full_sync_with_github(&self) {
        self.github_worker.trigger_and_wait().await;
        self.sync_with_db().await;
    }
}

#[derive(Debug)]
pub struct InstrumentedRwLock<T> {
    inner: RwLock<T>,
}

impl<T> InstrumentedRwLock<T> {
    pub fn new(data: T) -> Self {
        Self {
            inner: RwLock::new(data),
        }
    }

    pub async fn read(&self) -> tokio::sync::RwLockReadGuard<'_, T> {
        // let access = now();
        // let backtrace: String = Backtrace::force_capture()
        //     .to_string()
        //     .lines()
        //     .take(4)
        //     .collect::<Vec<_>>()
        //     .join("\n");
        // println!("Acquiring read guard for access {}:\n{}", access, backtrace);
        let guard = self.inner.read().await;
        // println!("Read guard acquired for access {}", access);
        guard
    }

    pub async fn write(&self) -> tokio::sync::RwLockWriteGuard<'_, T> {
        // let access = now();
        // let backtrace: String = Backtrace::force_capture()
        //     .to_string()
        //     .lines()
        //     .take(4)
        //     .collect::<Vec<_>>()
        //     .join("\n");
        // println!(
        //     "Acquiring write guard for access {}:\n{}",
        //     access, backtrace
        // );
        let guard = self.inner.write().await;
        // println!("Write guard acquired for access {}", access);
        guard
    }
}
