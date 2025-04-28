use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use futures::{stream, Stream, StreamExt};

use crate::container::commit::CommitContainer;
use crate::container::ContainerStatus;
use crate::db::{nano_id::NanoId, BuildResult, Deployment as DbDeployment};
use crate::hooks::StatusHooks;
use crate::sqlite_db::ProdSqliteDb;
use crate::Conf;
use crate::{
    container::Container,
    db::{Db, DeploymentWithProject},
    github::Github,
};

use super::config::Visibility;
use super::worker::WorkerHandle;

#[derive(Debug, Clone)]
pub(crate) struct Deployment {
    pub(crate) branch: String,
    pub(crate) default_branch: bool,
    pub(crate) sha: String,
    pub(crate) id: NanoId,
    pub(crate) project: NanoId,
    pub(crate) url_id: String,
    pub(crate) timestamp: i64,
    pub(crate) created: i64,
    pub(crate) forced_prod: bool, // TODO: review if im using this
    pub(crate) app_container: Arc<Container>, // FIXME: try to remove Arc, only needed to make access to socket/public generic
}

impl Deployment {
    pub(crate) fn iter_arc_containers(&self) -> impl Stream<Item = Arc<Container>> + Send + '_ {
        let containers: [Pin<Box<dyn Future<Output = Option<Arc<Container>>> + Send>>; 2] = [
            Box::pin(async { Some(self.app_container.clone()) }),
            Box::pin(async {
                self.app_container
                    .status
                    .read()
                    .await
                    .get_db_setup()
                    .map(|setup| setup.container.clone())
            }),
        ];
        stream::iter(containers).filter_map(|container| container)
    }

    pub(crate) async fn new(
        deployment: DeploymentWithProject,
        build_queue: WorkerHandle,
        github: Github,
        db: Db,
        project_db: &ProdSqliteDb,
    ) -> Self {
        let Conf { hostname, .. } = Conf::read_async().await; // TODO: take this from args?
        let db_url = deployment.get_libsql_url(&hostname);
        let DeploymentWithProject {
            deployment,
            project,
        } = deployment;
        let default_branch = deployment.is_default_branch();
        let DbDeployment {
            sha,
            env,
            branch,
            id,
            url_id,
            timestamp,
            created,
            config,
            ..
        } = deployment;

        let is_public = match config.get_visibility() {
            Visibility::Standard => default_branch,
            Visibility::Public => true,
            Visibility::Private => false,
        };

        let env = env.into();
        let hooks = StatusHooks::new(id.clone(), db, github.clone());

        let (inistial_status, build_result) = match deployment.result {
            Some(BuildResult::Failed) => (ContainerStatus::Failed, Some(BuildResult::Failed)),
            Some(BuildResult::Built) => (ContainerStatus::Built, Some(BuildResult::Built)),
            _ => (
                ContainerStatus::Queued {
                    trigger_access: None,
                },
                None,
            ),
        };

        let is_branch_deployment = !default_branch;
        let commit_container = CommitContainer::new(
            build_queue.clone(),
            hooks,
            github,
            project.repo_id,
            sha.clone(),
            id.clone(),
            env,
            project.root.clone(),
            is_branch_deployment,
            is_public,
            project_db,
            &db_url,
            inistial_status,
            build_result,
            config,
        );

        let forced_prod = project
            .prod_id
            .as_ref()
            .is_some_and(|prod_id| &id == prod_id);
        Self {
            branch,
            default_branch,
            sha,
            id,
            project: project.id.clone(),
            url_id,
            timestamp,
            created,
            forced_prod,
            app_container: commit_container.into(),
        }
    }
}
