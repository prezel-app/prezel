use std::{
    collections::{HashMap, HashSet},
    future::Future,
    sync::Arc,
};

use futures::{stream, Stream, StreamExt};
use tracing::error;

use crate::{
    container::{Container, ContainerStatus},
    db::{nano_id::NanoId, BuildResult, Db},
    github::Github,
    sqlite_db::{ProdSqliteDb, SqliteDbSetup},
    tls::CertificateStore,
};

use super::{deployment::Deployment, worker::WorkerHandle};

#[derive(Debug)]
pub(crate) struct DeploymentMap {
    pub(crate) dbs: HashMap<NanoId, ProdSqliteDb>, // project id -> prod db
    /// FIXME: this having a tuple (NanoId, String) as the key means every time I access I need to clone two strings. There has to be another way
    pub(crate) deployments: HashMap<(NanoId, String), Deployment>, // project id + deployment slug -> deployment
    /// values here used to be options, but removing them from the map should be enough
    pub(crate) prod: HashMap<NanoId, String>, // project id -> deployment slug
    pub(crate) names: HashMap<String, NanoId>, // project name -> project id
    pub(crate) certificates: CertificateStore,
    pub(crate) custom_domains: HashMap<String, NanoId>, // domain -> project id
}

impl DeploymentMap {
    pub(crate) fn new(store: CertificateStore) -> Self {
        Self {
            dbs: Default::default(),
            deployments: Default::default(),
            prod: Default::default(),
            names: Default::default(),
            custom_domains: Default::default(),
            certificates: store,
        }
    }

    #[tracing::instrument]
    pub(crate) fn iter_containers(&self) -> impl Stream<Item = Arc<Container>> + Send + '_ {
        let prod_dbs = self.dbs.values().map(|db| db.setup.container.clone());
        let deployments = stream::iter(self.deployments.iter())
            .flat_map(|(_, deployment)| deployment.iter_arc_containers());
        stream::iter(prod_dbs).chain(deployments)
    }

    #[tracing::instrument]
    pub(crate) fn get_deployment_by_name(
        &self,
        project: &String,
        deployment: String,
    ) -> Option<&Deployment> {
        let id = self.names.get(project)?;
        self.get_deployment_by_id(id.clone(), deployment)
    }

    #[tracing::instrument]
    pub(crate) fn get_deployment_by_id(
        &self,
        project: NanoId,
        deployment: String,
    ) -> Option<&Deployment> {
        self.deployments.get(&(project, deployment))
    }

    #[tracing::instrument]
    pub(crate) fn has_deployment_id(&self, id: &NanoId) -> bool {
        self.deployments
            .values()
            .any(|deployment| &deployment.id == id)
    }

    #[tracing::instrument]
    fn get_prod_from_id(&self, id: &NanoId) -> Option<&Deployment> {
        let prod_id = self.prod.get(id)?;
        self.deployments.get(&(id.clone(), prod_id.to_string()))
    }

    #[tracing::instrument]
    pub(crate) fn get_prod(&self, project: &str) -> Option<&Deployment> {
        let project_id = self.names.get(project)?;
        self.get_prod_from_id(project_id)
    }

    #[tracing::instrument]
    pub(crate) fn get_prod_db(&self, id: &NanoId) -> Option<SqliteDbSetup> {
        self.dbs.get(id).map(|db| db.setup.clone())
    }

    #[tracing::instrument]
    pub(crate) fn get_custom_domain(&self, domain: &str) -> Option<&Deployment> {
        let project = self.custom_domains.get(domain)?;
        self.get_prod_from_id(project)
    }

    // TODO: this is currently kind of a mutex because is getting &mut,
    // but if that ever changes, I might need a way to make it mutex again
    #[tracing::instrument]
    pub(crate) async fn read_db_and_build_updates(
        &mut self,
        build_queue: &WorkerHandle,
        github: &Github,
        db: &Db,
    ) -> anyhow::Result<()> {
        let required_deployments = db.get_deployments_with_project().await?.collect::<Vec<_>>();

        let required_ids = required_deployments
            .iter()
            .map(|dep| (dep.project.id.clone(), dep.deployment.url_id.clone()))
            .collect::<HashSet<_>>();

        // sync map.names
        let projects = required_deployments
            .iter()
            .map(|dep| (dep.project.id.clone(), dep.project.clone()))
            .collect::<HashMap<_, _>>();
        self.names = projects
            .iter()
            .map(|(id, project)| (project.name.clone(), id.clone()))
            .collect();

        // sync map.custom_domains
        self.custom_domains = projects
            .iter()
            .flat_map(|(id, project)| {
                project
                    .custom_domains
                    .iter()
                    .map(|domain| (domain.to_owned(), id.clone()))
            })
            .collect();

        // sync map.dbs
        for (project_id, _) in &projects {
            if !self.dbs.contains_key(project_id) {
                // TODO: remove unwrap
                self.dbs.insert(
                    project_id.clone(),
                    ProdSqliteDb::new(&project_id, build_queue.clone()).unwrap(),
                );
            }
        }

        // sync map.certificates
        let required_certificates = self.custom_domains.keys();
        for domain in required_certificates {
            if !self.certificates.has_domain(domain) {
                self.certificates.insert_domain(domain.to_owned());
            }
            // TODO: should also remove unneeded certificates?
        }

        // sync map.deployments
        for deployment in required_deployments {
            if !self.deployments.contains_key(&(
                deployment.project.id.clone(),
                deployment.deployment.url_id.clone(),
            )) {
                let project = deployment.project.id.clone();
                let url_id = deployment.deployment.url_id.clone();
                if let Some(prod_db) = self.dbs.get(&project) {
                    let deployment = Deployment::new(
                        deployment,
                        build_queue.clone(),
                        github.clone(),
                        db.clone(),
                        prod_db,
                    )
                    .await;
                    self.deployments.insert((project, url_id), deployment);
                } else {
                    error!("illegal state, no prod bd found for deployment"); // TODO: make this impossible
                }
            }
        }
        let existing_ids = self.deployments.keys().cloned().collect::<Vec<_>>();
        for id in existing_ids {
            if !required_ids.contains(&id) {
                self.deployments.remove(&id);
            }
        }

        // sync map.prod
        self.prod = stream::iter(projects)
            .map(|(id, _)| {
                let project_deployments = self
                    .deployments
                    .iter()
                    .map(|(_, deployment)| deployment)
                    .filter(|deployment| deployment.project == id && deployment.default_branch)
                    .map(|deployment| {
                        (
                            deployment.app_container.clone(),
                            deployment.created,
                            deployment.url_id.clone(),
                        )
                    })
                    .collect::<Vec<_>>();
                (id, project_deployments)
            })
            .filter_map(|(id, project_deployments)| async move {
                // TODO: bear in mind prod id saved in the db
                let latest_prod_id = project_deployments
                    .iter()
                    .max_by_key(|(_, created, _)| created)
                    .map(|(_, _, slug)| slug.clone());
                let latest_successful_prod_id =
                        stream::iter(project_deployments)
                            .filter(|(app_container, _, _)| {
                                let app_container = app_container.clone();
                                async move {
                                    *app_container.result.read().await == Some(BuildResult::Built)
                                }
                            })
                            .fold((0, None), |current, (_, created, url_id)| async move {
                                if created > current.0 {
                                    (created, Some(url_id.clone()))
                                } else {
                                    current
                                }
                            })
                            .await
                            .1;
                let prod_id = latest_successful_prod_id.or(latest_prod_id)?;
                Some((id, prod_id))
            })
            .collect()
            .await;
        // TODO: lots of clones going on above, the code below seems so close to work...
        // self.prod = stream::iter(projects)
        //     .filter_map(|(id, _)| async {
        //         // TODO: bear in mind prod id saved in the db
        //         let project_deployments = self
        //             .deployments
        //             .iter()
        //             .map(|(_, deployment)| deployment)
        //             .filter(|deployment| deployment.project == id);
        //         let prod_id = stream::iter(project_deployments)
        //             .filter(|deployment| async {
        //                 deployment.app_container.status.read().await.is_built()
        //             })
        //             .fold((0, None), |current, deployment| async {
        //                 if deployment.timestamp > current.0 {
        //                     (deployment.timestamp, Some(deployment.url_id.clone()))
        //                 } else {
        //                     current
        //                 }
        //             })
        //             .await
        //             .1?;
        //         Some((id, prod_id))
        //     })
        //     .collect()
        //     .await;

        // force build prod containers
        for deployment in self.iter_prod_deployments() {
            let status = deployment.app_container.status.read().await.clone();
            match status {
                // the logic to put containers into the queue is a bit duplicated.
                // Maybe everything related to putting containers into the Queue should be here,
                // but that means I need an additional status
                ContainerStatus::Built { .. } => {
                    deployment.app_container.enqueue().await;
                }
                _ => {}
            }
        }

        // downgrade unused containers
        for container in self.get_all_non_prod_containers().await {
            container.downgrade_if_unused().await;
        }

        Ok(())
    }

    #[tracing::instrument]
    fn iter_prod_deployments(&self) -> impl Iterator<Item = &Deployment> {
        self.names
            .keys()
            .filter_map(|project| self.get_prod(project))
    }

    // TODO: once I get this to be Send, change signature back to async fn ...
    #[tracing::instrument]
    fn get_all_non_prod_containers(&self) -> impl Future<Output = Vec<Arc<Container>>> + Send + '_ {
        let prod_deployment_ids = self
            .iter_prod_deployments()
            .map(|deployment| deployment.id.clone())
            .collect::<Vec<_>>();
        let all_containers_from_non_prod_deployments = stream::iter(
            self.deployments
                .values()
                .filter(move |deployment| !prod_deployment_ids.contains(&deployment.id)),
        )
        .flat_map(|deployment| deployment.iter_arc_containers());

        let db_containers_from_prod_deployments = stream::iter(self.iter_prod_deployments())
            .filter_map(|deployment| async {
                // FIXME: some boilerplate here, could have deployment.get_db_container
                deployment
                    .app_container
                    .status
                    .read()
                    .await
                    .get_db_setup()
                    .map(|setup| setup.container.clone())
            });

        all_containers_from_non_prod_deployments
            .chain(db_containers_from_prod_deployments)
            .collect()
    }
}
