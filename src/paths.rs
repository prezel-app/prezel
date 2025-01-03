use std::{
    env,
    fs::create_dir_all,
    path::{Path, PathBuf},
};

const DB_NAME: &str = "app.db";
const LOG_FILE: &str = "log";

pub(crate) fn get_instance_db_path() -> PathBuf {
    get_container_root().join(DB_NAME)
}

pub(crate) fn get_instance_log_dir() -> PathBuf {
    get_container_root().join(LOG_FILE)
}

#[derive(Debug, Clone)]
pub(crate) struct HostFile {
    relative_folder_path: PathBuf,
    filename: String,
}

impl HostFile {
    pub(crate) fn new(relative_folder_path: PathBuf, filename: impl AsRef<str>) -> Self {
        // TODO: panic if relative_folder_path is not relative
        Self {
            relative_folder_path,
            filename: filename.as_ref().to_owned(),
        }
    }
    pub(crate) fn get_host_folder(&self) -> PathBuf {
        get_host_root().join(&self.relative_folder_path)
    }

    // pub(crate) fn get_host_file(&self) -> PathBuf {
    //     self.get_host_folder().join(&self.filename)
    // }

    pub(crate) fn get_container_folder(&self) -> PathBuf {
        let path = get_container_root().join(&self.relative_folder_path);
        create_dir_all(&path).unwrap(); // TODO: is this good enough?
        path
    }

    pub(crate) fn get_container_file(&self) -> PathBuf {
        self.get_container_folder().join(&self.filename)
    }
}

// const HOST_ROOT: &'static str = "~/prezel";
const CONTAINER_ROOT: &'static str = "/opt/prezel";

fn get_host_root() -> PathBuf {
    env::var("PREZEL_HOME").unwrap().into()
}

// TODO: give a better name to this ?
pub(crate) fn get_container_root() -> &'static Path {
    Path::new(CONTAINER_ROOT)
}
