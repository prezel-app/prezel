use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use jsonwebtoken::{DecodingKey, EncodingKey};
use ring::signature::{Ed25519KeyPair, KeyPair};
use serde::Serialize;

use crate::{
    container::{sqld::SqldContainer, Container},
    deployments::worker::WorkerHandle,
    paths::HostFile,
};

#[derive(Debug)]
pub(crate) struct ProdSqliteDb {
    pub(crate) setup: SqliteDbSetup,
    project_folder: PathBuf,
    build_queue: WorkerHandle,
}

impl ProdSqliteDb {
    // TODO: build_queue is needed in case the container needs to trigger its own build because
    // someone is trying to access it. But this never happens for sqld containers...
    pub(crate) fn new(project_id: i64, build_queue: WorkerHandle) -> anyhow::Result<Self> {
        let project_folder = Path::new("sqlite").join(project_id.to_string());
        let file = HostFile::new(project_folder.clone(), "main.db");

        let main_db_path = file.get_container_file();
        if !main_db_path.exists() {
            std::fs::File::create_new(&main_db_path)?;
        }

        let auth = SqldAuth::generate();
        let container = SqldContainer::new(file.clone(), &auth.key, build_queue.clone()).into();

        Ok(Self {
            setup: SqliteDbSetup {
                file,
                container,
                auth,
            },
            project_folder,
            build_queue,
        })
    }

    pub(crate) fn branch(&self, deployment_id: i64) -> BranchSqliteDb {
        let path = self.project_folder.join(deployment_id.to_string());
        let branch_file = HostFile::new(path, "preview.db");
        BranchSqliteDb {
            base_file: self.setup.file.clone(),
            branch_file,
            build_queue: self.build_queue.clone(),
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct BranchSqliteDb {
    base_file: HostFile,
    pub(crate) branch_file: HostFile,
    build_queue: WorkerHandle,
}

impl BranchSqliteDb {
    pub(crate) async fn setup(&self) -> anyhow::Result<SqliteDbSetup> {
        tokio::fs::copy(
            self.base_file.get_container_file(),
            self.branch_file.get_container_file(),
        )
        .await?;
        let auth = SqldAuth::generate();
        let container = SqldContainer::new(
            self.branch_file.clone(),
            &auth.key,
            self.build_queue.clone(),
        )
        .into();
        Ok(SqliteDbSetup {
            file: self.branch_file.clone(),
            container,
            auth,
        })
    }
}

#[derive(Debug, Clone)]
pub(crate) struct SqliteDbSetup {
    pub(crate) file: HostFile,
    pub(crate) container: Arc<Container>,
    pub(crate) auth: SqldAuth,
}

#[derive(Debug, Clone)]
pub(crate) struct SqldAuth {
    pub(crate) token: String,
    key: String,
}

impl SqldAuth {
    fn generate() -> Self {
        let doc = Ed25519KeyPair::generate_pkcs8(&ring::rand::SystemRandom::new()).unwrap();
        let encoding_key = EncodingKey::from_ed_der(doc.as_ref());
        let pair = Ed25519KeyPair::from_pkcs8(doc.as_ref()).unwrap();
        let key = URL_SAFE_NO_PAD.encode(pair.public_key().as_ref());

        let token = Token {
            id: None,
            a: None,
            p: None,
            exp: None,
        };
        let token = encode(&token, &encoding_key);

        Self { key, token }
    }
}

// usize is not the actual type accepted by sqld. Have a look at libsql repo for more info
#[derive(serde::Deserialize, serde::Serialize, Debug, Default)]
pub struct Token {
    #[serde(default)]
    id: Option<usize>,
    #[serde(default)]
    a: Option<usize>,
    #[serde(default)]
    pub(crate) p: Option<usize>,
    #[serde(default)]
    exp: Option<usize>,
}

fn encode<T: Serialize>(claims: &T, key: &EncodingKey) -> String {
    let header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::EdDSA);
    jsonwebtoken::encode(&header, &claims, key).unwrap()
}
