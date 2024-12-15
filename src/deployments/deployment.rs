use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::container::commit::CommitContainer;
use crate::container::prisma::PrismaContainer;
use crate::container::ContainerStatus;
use crate::db::{BuildResult, Deployment as DbDeployment};
use crate::deployment_hooks::StatusHooks;
use crate::paths::HostFile;
use crate::{
    container::Container,
    db::{Db, DeploymentWithProject},
    github::Github,
};

use super::label::Label;
use super::manager::Manager;
use super::worker::WorkerHandle;

#[derive(Debug)]
pub(crate) struct Deployment {
    pub(crate) branch: String,
    pub(crate) default_branch: bool,
    pub(crate) sha: String,
    pub(crate) id: i64,
    pub(crate) project: i64,
    pub(crate) url_id: String,
    pub(crate) timestamp: i64,
    pub(crate) created: i64,
    // pub(crate) target_hostname: String,
    // pub(crate) deployment_hostname: String,
    // pub(crate) prisma_hostname: String,
    pub(crate) forced_prod: bool, // TODO: review if im using this
    pub(crate) app_container: Arc<Container>, // FIXME: try to remove Arc, only needed to make access to socket/public generic
    pub(crate) prisma_container: Arc<Container>,
}

impl Deployment {
    fn get_all_containers(&self) -> impl Iterator<Item = &Container> {
        [self.app_container.as_ref(), self.prisma_container.as_ref()].into_iter()
    }

    // TODO:  try to merge this with the one above?
    pub(crate) fn iter_arc_containers(&self) -> impl Iterator<Item = Arc<Container>> {
        [self.app_container.clone(), self.prisma_container.clone()].into_iter()
    }

    pub(crate) fn new(
        deployment: DeploymentWithProject,
        build_queue: WorkerHandle,
        github: Github,
        db: Db,
    ) -> Self {
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
            ..
        } = deployment;

        let env = env.into();

        let dbs_path = get_dbs_path(project.id);
        let cloned_db_file = if default_branch {
            None
        } else {
            let path = dbs_path.join(id.to_string());
            Some(HostFile::new(path, "preview.db"))
        };
        let main_db_file = HostFile::new(dbs_path, "main.db");

        // TODO: this boilerplate is also in CommitContainer::new()
        let db_file = cloned_db_file
            .clone()
            .unwrap_or_else(|| main_db_file.clone());

        let hooks = StatusHooks::new(db, id);

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

        let commit_container = CommitContainer::new(
            build_queue.clone(),
            hooks,
            github,
            project.repo_id.clone(),
            sha.clone(),
            id,
            env,
            project.root.clone(),
            default_branch, // public
            main_db_file,
            cloned_db_file,
            inistial_status,
            build_result,
        );
        let prisma_container = PrismaContainer::new(db_file, build_queue);

        Self {
            branch,
            default_branch,
            sha,
            id,
            project: project.id,
            url_id,
            timestamp,
            created,
            forced_prod: project.prod_id.is_some_and(|prod_id| id == prod_id),
            app_container: commit_container.into(),
            prisma_container: prisma_container.into(),
        }
    }

    pub(crate) fn get_app_hostname(&self, box_domain: &str, project_name: &str) -> String {
        Label::Deployment {
            project: project_name.to_string(),
            deployment: self.url_id.to_string(),
        }
        .format_hostname(box_domain)
    }

    pub(crate) fn get_prod_hostname(&self, box_domain: &str, project_name: &str) -> String {
        Label::Prod {
            project: project_name.to_string(),
        }
        .format_hostname(box_domain)
    }

    pub(crate) fn get_db_hostname(&self, box_domain: &str, project_name: &str) -> String {
        Label::Db {
            project: project_name.to_string(),
            deployment: self.url_id.to_string(),
        }
        .format_hostname(box_domain)
    }
}

fn get_dbs_path(project_id: i64) -> PathBuf {
    Path::new("sqlite").join(project_id.to_string()) // FIXME: should use the id!!!!!!!!!!
}
