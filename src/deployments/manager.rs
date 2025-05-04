use std::{collections::HashMap, sync::Arc, time::Duration};

use pingora::tls;
use tokio::sync::RwLock;

use crate::{
    container::Container,
    db::{nano_id::NanoId, Db},
    github::Github,
    label::Label,
    sqlite_db::SqliteDbSetup,
    tls::{CertificateStore, TlsState},
    utils::LogError,
};

use super::{
    deployment::Deployment,
    map::DeploymentMap,
    worker::{Worker, WorkerHandle},
    workers::{build::BuildWorker, docker::DockerWorker, files::FilesWorker, github::GithubWorker},
};

#[derive(Clone, Debug)]
pub(crate) struct Manager {
    pub(crate) box_domain: String,
    deployments: Arc<InstrumentedRwLock<DeploymentMap>>,
    build_worker: Arc<WorkerHandle>,
    github_worker: Arc<WorkerHandle>,
    docker_worker: Arc<WorkerHandle>,
    files_worker: Arc<WorkerHandle>,
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

        let deployments_clone = deployments.clone();
        let files_worker = FilesWorker::start(|_| FilesWorker {
            map: deployments_clone,
        })
        .into();

        let manager = Self {
            deployments,
            box_domain,
            build_worker,
            github_worker,
            docker_worker,
            files_worker,
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

    pub(crate) async fn get_main_certificate(&self) -> anyhow::Result<tls::x509::X509> {
        let main_cert = self
            .deployments
            .read()
            .await
            .certificates
            .get_default_certificate();
        main_cert.load_pem()
    }

    pub(crate) async fn get_custom_domain_certificates(
        &self,
    ) -> anyhow::Result<HashMap<String, tls::x509::X509>> {
        let domains = self.deployments.read().await.certificates.get_all_domains();
        let certs = domains.into_iter().filter_map(|(domain, state)| {
            if let TlsState::Ready(cert) = state {
                Some((domain, cert))
            } else {
                None
            }
        });
        certs
            .map(|(domain, cert)| cert.load_pem().map(|pem| (domain, pem)))
            .collect()
    }

    #[tracing::instrument]
    pub(crate) async fn get_container_by_hostname(
        &self,
        hostname: &str,
    ) -> Option<(Arc<Container>, bool)> {
        let container = {
            let deployments = self.deployments.read().await;
            let deployment = deployments.get_custom_domain(hostname);
            deployment.map(|deployment| deployment.app_container.clone())
        };
        if let Some(container) = container {
            Some((container, false))
        } else {
            let label = Label::strip_from_domain(hostname, &self.box_domain).ok()?;
            let insert_enabled = label.insert_enabled();
            dbg!(&label);
            dbg!(&insert_enabled);
            let container = self.get_container_by_label(label).await?;
            Some((container, insert_enabled))
        }
    }

    #[tracing::instrument]
    async fn get_container_by_label(&self, label: Label) -> Option<Arc<Container>> {
        let map = self.deployments.read().await;
        match label {
            Label::Prod { project } => {
                let deployment = map.get_prod(&project)?;
                Some(deployment.app_container.clone())
            }
            Label::Deployment {
                project,
                deployment,
            }
            | Label::DeploymentInsert {
                project,
                deployment,
            } => {
                let deployment = map.get_deployment_by_name(&project, deployment)?;
                Some(deployment.app_container.clone())
            }
            Label::BranchDb {
                project,
                deployment,
            } => {
                let deployment = map.get_deployment_by_id(project, deployment)?;
                let status = &deployment.app_container.status;
                status
                    .read()
                    .await
                    .get_db_setup()
                    .map(|setup| setup.container.clone())
            }
            Label::ProdDb { project } => map
                .get_prod_db(&project)
                .map(|setup| setup.container.clone()),
        }
    }

    #[tracing::instrument]
    pub(crate) async fn get_deployment(&self, id: &NanoId) -> Option<Deployment> {
        let map = self.deployments.read().await;
        map.deployments
            .values()
            .find(|deployment| &deployment.id == id)
            .cloned()
    }

    #[tracing::instrument]
    pub(crate) async fn get_prod_deployment(&self, project: &NanoId) -> Option<Deployment> {
        let map = self.deployments.read().await;
        let prod_id = map.prod.get(project)?;
        map.deployments
            .get(&(project.clone(), prod_id.to_owned()))
            .cloned()
    }

    #[tracing::instrument]
    pub(crate) async fn get_prod_db(&self, project: &NanoId) -> Option<SqliteDbSetup> {
        self.deployments.read().await.get_prod_db(project)
    }

    #[tracing::instrument]
    pub(crate) async fn get_prod_url_id(&self, project: &NanoId) -> Option<String> {
        let map = self.deployments.read().await;
        Some(map.prod.get(project)?.to_owned())
    }

    #[tracing::instrument]
    pub(crate) async fn sync_with_db(&self) {
        self.deployments
            .write()
            .await
            .read_db_and_build_updates(&self.build_worker, &self.github, &self.db)
            .await
            .ignore_logging();
        self.build_worker.trigger();
        self.docker_worker.trigger();
        self.files_worker.trigger();
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
