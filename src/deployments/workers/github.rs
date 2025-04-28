use tracing::error;

use crate::{
    db::{Db, InsertDeployment, Project},
    deployments::{config::DeploymentConfig, worker::Worker},
    github::{Commit, Github},
    utils::LogError,
};

#[derive(Clone, Debug)]
pub(crate) struct GithubWorker {
    pub(crate) github: Github,
    pub(crate) db: Db,
}

impl Worker for GithubWorker {
    #[tracing::instrument]
    fn work(&self) -> impl std::future::Future<Output = ()> + Send {
        async {
            // if there is some error when trying to read from the db, we simply skip the work
            let _ = self.github_work().await.inspect_err(|e| error!("{e}"));
        }
    }
}

impl GithubWorker {
    #[tracing::instrument]
    async fn github_work(&self) -> anyhow::Result<()> {
        for Project {
            repo_id,
            env,
            id,
            root,
            name,
            ..
        } in self.db.get_projects().await?
        {
            let commit = self.get_default_branch_and_latest_commit(repo_id).await;
            match commit {
                Ok((default_branch, commit)) => {
                    let deployment = InsertDeployment {
                        env: env.to_owned(),
                        sha: commit.sha,
                        timestamp: commit.timestamp,
                        branch: default_branch,
                        default_branch: 1, // TODO: abstract this as a bool
                        project: id.clone(),
                        result: None,
                    };
                    self.add_deployment_to_db_if_missing(deployment, repo_id, &root, &name)
                        .await
                        .ignore_logging();
                }
                Err(error) => error!("{error}"),
            }

            let pull_results = self.github.get_open_pulls(repo_id).await;
            let pulls = pull_results
                .inspect_err(|error| error!("{error}"))
                .unwrap_or(vec![]);
            for pull in pulls {
                let branch = pull.head.ref_field;
                // FIXME: some duplicated code in here as in above
                match self.github.get_latest_commit(repo_id, &branch).await {
                    Ok(commit) => {
                        let deployment = InsertDeployment {
                            env: env.to_owned(),
                            sha: commit.sha,
                            timestamp: commit.timestamp,
                            branch,
                            default_branch: 0, // TODO: abstract this as a bool
                            project: id.clone(),
                            result: None,
                        };
                        self.add_deployment_to_db_if_missing(deployment, repo_id, &root, &name)
                            .await
                            .ignore_logging();
                    }
                    Err(error) => error!("{error}"),
                }
            }
        }
        Ok(())
    }
}

impl GithubWorker {
    #[tracing::instrument]
    async fn get_default_branch_and_latest_commit(
        &self,
        repo_id: i64,
    ) -> anyhow::Result<(String, Commit)> {
        let default_branch = self.github.get_default_branch(repo_id).await?;
        let commit = self
            .github
            .get_latest_commit(repo_id, &default_branch)
            .await?;
        Ok((default_branch, commit))
    }

    #[tracing::instrument]
    async fn add_deployment_to_db_if_missing(
        &self,
        mut deployment: InsertDeployment, // FIXME: don't like having this mut here
        repo_id: i64,
        root: &str,
        app_name: &str,
    ) -> anyhow::Result<()> {
        let exists = self
            .db
            .hash_exists_for_project(&deployment.sha, &deployment.project)
            .await?;
        if !exists {
            let (config, error) = match DeploymentConfig::fetch_from_repo(
                &self.github,
                repo_id,
                &deployment.sha,
                root,
                app_name,
            )
            .await
            {
                Ok(config) => (config.unwrap_or_default(), None),
                Err(error) => {
                    deployment.result = Some(crate::db::BuildResult::Failed);
                    (Default::default(), Some(error))
                }
            };
            if let Some(error) = error {
                let id = self.db.insert_deployment(deployment, config.into()).await?;
                self.db
                    .insert_deployment_build_log(&id, &error.to_string(), true)
                    .await
                    .ignore_logging();
            }
        }
        Ok(())
    }
}
