use tracing::error;

use crate::{
    db::{Db, InsertDeployment, Project},
    deployments::worker::Worker,
    github::{Commit, Github},
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
            for Project {
                repo_id, env, id, ..
            } in self.db.get_projects().await
            {
                let commit = get_default_branch_and_latest_commit(&self.github, repo_id).await;
                match commit {
                    Ok((default_branch, commit)) => {
                        let deployment = InsertDeployment {
                            env: env.to_owned(),
                            sha: commit.sha,
                            timestamp: commit.timestamp,
                            branch: default_branch,
                            default_branch: 1, // TODO: abstract this as a bool
                            project: id.clone(),
                        };
                        add_deployment_to_db_if_missing(&self.db, deployment).await;
                    }
                    Err(error) => error!("{error:?}"),
                }

                let pull_results = self.github.get_open_pulls(repo_id).await;
                let pulls = pull_results
                    .inspect_err(|error| error!("{error:?}"))
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
                            };
                            add_deployment_to_db_if_missing(&self.db, deployment).await;
                        }
                        Err(error) => error!("{error:?}"),
                    }
                }
            }
        }
    }
}

#[tracing::instrument]
async fn get_default_branch_and_latest_commit(
    github: &Github,
    repo_id: i64,
) -> anyhow::Result<(String, Commit)> {
    let default_branch = github.get_default_branch(repo_id).await?;
    let commit = github.get_latest_commit(repo_id, &default_branch).await?;
    Ok((default_branch, commit))
}

#[tracing::instrument]
async fn add_deployment_to_db_if_missing(db: &Db, deployment: InsertDeployment) {
    if !db
        .hash_exists_for_project(&deployment.sha, &deployment.project)
        .await
    {
        let _ = db
            .insert_deployment(deployment)
            .await
            .inspect_err(|e| error!("{e:?}"));
    }
}
