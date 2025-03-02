use anyhow::anyhow;
use flate2::read::GzDecoder;
use http_body_util::BodyExt;
use octocrab::{
    models::{issues::Comment, pulls::PullRequest, CommentId, IssueState, Repository},
    params::{
        checks::{CheckRunConclusion, CheckRunStatus},
        repos::Commitish,
    },
    Octocrab,
};
use std::{collections::HashMap, io::Cursor, path::Path, sync::Arc};
use tar::Archive;
use tokio::sync::{Mutex, MutexGuard, RwLock};

use crate::{provider, utils::now};

pub(crate) struct Commit {
    pub(crate) timestamp: i64,
    pub(crate) sha: String,
}

#[derive(Debug, Clone)]
struct Token {
    secret: String,
    millis: i64,
}

#[derive(Clone, Debug)]
pub(crate) struct Github {
    tokens: Arc<RwLock<HashMap<i64, Token>>>,
    bot_mutex: Arc<Mutex<()>>,
}

impl Github {
    pub(crate) async fn new() -> Self {
        Self {
            tokens: Default::default(),
            bot_mutex: Mutex::new(()).into(),
        }
    }

    #[tracing::instrument]
    pub(crate) async fn get_open_pulls(&self, repo_id: i64) -> anyhow::Result<Vec<PullRequest>> {
        let crab = self.get_crab(repo_id).await?;
        let (owner, name) = self.get_owner_and_name(repo_id).await?;
        let pulls = crab.pulls(owner, name).list().send().await?;
        Ok(pulls
            .into_iter()
            .filter(|pull| pull.state == Some(IssueState::Open))
            .collect())
    }

    #[tracing::instrument]
    pub(crate) async fn get_repo(&self, id: i64) -> anyhow::Result<Option<Repository>> {
        let crab = self.get_crab(id).await?;
        Ok(crab.get(format!("/repositories/{id}"), None::<&()>).await?)
    }

    // #[tracing::instrument]
    // pub(crate) async fn get_pull(&self, repo_id: i64, number: u64) -> anyhow::Result<PullRequest> {
    //     let crab = self.get_crab(repo_id).await?;
    //     let (owner, name) = self.get_owner_and_name(repo_id).await?;
    //     Ok(crab.pulls(owner, name).get(number).await?)
    // }

    #[tracing::instrument]
    pub(crate) async fn get_default_branch(&self, repo_id: i64) -> anyhow::Result<String> {
        let crab = self.get_crab(repo_id).await?;
        let (owner, name) = self.get_owner_and_name(repo_id).await?;
        let repository = crab.repos(owner, name).get().await?;
        Ok(repository.default_branch.unwrap())
    }

    #[tracing::instrument]
    pub(crate) async fn get_latest_commit(
        &self,
        repo_id: i64,
        branch: &str,
    ) -> anyhow::Result<Commit> {
        let crab = self.get_crab(repo_id).await?;
        let (owner, name) = self.get_owner_and_name(repo_id).await?;
        let commit = crab.commits(owner, name).get(branch).await?;
        let timestamp = commit
            .commit
            .committer
            .and_then(|commiter| commiter.date.map(|date| date.timestamp_millis()))
            .unwrap_or(now()); // FIXME: if Im using this timestamp just for the UI, better have a None here, on the other hand if I'm using this for ordering, this might be dangerous
        Ok(Commit {
            timestamp,
            sha: commit.sha,
        })
    }

    #[tracing::instrument]
    pub(crate) async fn download_commit(
        &self,
        repo_id: i64,
        sha: &str,
        path: &Path,
    ) -> anyhow::Result<()> {
        let crab = self.get_crab(repo_id).await?;
        let (owner, name) = self.get_owner_and_name(repo_id).await?;
        let response = crab
            .repos(owner, name)
            .download_tarball(sha.to_owned())
            .await
            .unwrap();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        let content = Cursor::new(bytes);
        let mut archive = Archive::new(GzDecoder::new(content));
        for entry in archive.entries().unwrap() {
            let mut entry = entry.unwrap();
            let entry_path = entry.path().unwrap();
            let mut components = entry_path.components();
            components.next();
            let inner_path = components.as_path();
            entry.unpack(&path.join(inner_path)).unwrap();
        }
        Ok(())
    }

    #[tracing::instrument]
    pub(crate) async fn download_file(
        &self,
        repo_id: i64,
        sha: &str,
        path: &str,
    ) -> anyhow::Result<String> {
        let crab = self.get_crab(repo_id).await?;
        let (owner, name) = self.get_owner_and_name(repo_id).await?;
        let mut contents = crab
            .repos(owner, name)
            .get_content()
            .path(path)
            .r#ref(sha)
            .send()
            .await?;
        let content = contents.take_items().pop().ok_or(anyhow!("no content"))?;
        let decoded = content.decoded_content();
        decoded.ok_or(anyhow!("invalid content"))
    }

    // TODO: make this receive crab as argument
    #[tracing::instrument]
    async fn get_owner_and_name(&self, id: i64) -> anyhow::Result<(String, String)> {
        let repo = self.get_repo(id).await?;
        let repo = repo.ok_or_else(|| anyhow!("Repo not found"))?;
        Ok((repo.owner.unwrap().login, repo.name))
    }

    #[tracing::instrument]
    async fn get_crab(&self, repo_id: i64) -> anyhow::Result<Octocrab> {
        let secret = self.update_token(repo_id).await?;
        Ok(octocrab::OctocrabBuilder::default()
            .user_access_token(secret)
            .build()
            .unwrap())
    }

    #[tracing::instrument]
    async fn update_token(&self, repo_id: i64) -> anyhow::Result<String> {
        let mut tokens = self.tokens.write().await;
        let token = tokens.get(&repo_id);
        match token {
            Some(token) if !is_token_too_old(token) => Ok(token.secret.clone()),
            _ => {
                let token = Token {
                    secret: provider::get_github_token(repo_id).await?,
                    millis: now(),
                };
                tokens.insert(repo_id.to_owned(), token.clone());
                Ok(token.secret)
            }
        }
    }

    pub(crate) async fn allocate_bot(&self) -> GithubBot {
        GithubBot {
            github: self.clone(),
            guard: self.bot_mutex.lock().await,
        }
    }
}

#[derive(Debug)]
pub(crate) struct GithubBot<'a> {
    github: Github,
    guard: MutexGuard<'a, ()>,
}

impl<'a> GithubBot<'a> {
    #[tracing::instrument]
    pub(crate) async fn upsert_pull_check(
        &self,
        repo_id: i64,
        sha: &str,
        check_name: &str,
        status: CheckRunStatus,
        conclusion: Option<CheckRunConclusion>,
    ) -> anyhow::Result<()> {
        let crab = self.github.get_crab(repo_id).await?;
        let (owner, name) = self.github.get_owner_and_name(repo_id).await?;
        let check_handler = crab.checks(owner, name);
        let checks = check_handler
            .list_check_runs_for_git_ref(Commitish(sha.into()))
            .send()
            .await
            .unwrap();

        let app_check = checks
            .check_runs
            .iter()
            .find(|check| check.name == check_name);

        match app_check {
            Some(check) => {
                let mut builder = check_handler.update_check_run(check.id).status(status);
                if let Some(conclusion) = conclusion {
                    builder = builder.conclusion(conclusion);
                }
                builder.send().await.unwrap();
            }
            None => {
                let mut builder = check_handler
                    .create_check_run(check_name, sha)
                    // .details_url(details_url) // TODO: add this -> have a look at the vercel details URL
                    .status(status);

                if let Some(conclusion) = conclusion {
                    builder = builder.conclusion(conclusion);
                }
                builder.send().await.unwrap();
            }
        }
        Ok(())
    }

    pub(crate) async fn read_pull_comment_with_prefix(
        &self,
        repo_id: i64,
        pull: u64,
        prefix: &str,
    ) -> anyhow::Result<Option<Comment>> {
        let crab = self.github.get_crab(repo_id).await?;
        let (owner, name) = self.github.get_owner_and_name(repo_id).await?;
        let comments = crab
            .issues(&owner, &name)
            .list_comments(pull)
            .send()
            .await?;
        let comment = comments.items.into_iter().find(|comment| {
            let body = comment.body.as_ref();
            body.is_some_and(|body| body.starts_with(prefix))
        });
        Ok(comment)
    }

    #[tracing::instrument]
    pub(crate) async fn create_pull_comment(
        &self,
        repo_id: i64,
        pull: u64,
        content: &str,
    ) -> anyhow::Result<()> {
        let crab = self.github.get_crab(repo_id).await?;
        let (owner, name) = self.github.get_owner_and_name(repo_id).await?;
        crab.issues(owner, name)
            .create_comment(pull, content)
            .await?;
        Ok(())
    }

    #[tracing::instrument]
    pub(crate) async fn update_pull_comment(
        &self,
        repo_id: i64,
        pull: u64,
        comment: CommentId,
        content: &str,
    ) -> anyhow::Result<()> {
        let crab = self.github.get_crab(repo_id).await?;
        let (owner, name) = self.github.get_owner_and_name(repo_id).await?;
        crab.issues(owner, name)
            .update_comment(comment, content)
            .await?;
        Ok(())
    }
}

fn is_token_too_old(token: &Token) -> bool {
    let age = now() - token.millis;
    age > 30 * 60 * 1000
}
