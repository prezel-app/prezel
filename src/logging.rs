use std::{
    fs::{self, File},
    io::{self, Write},
    path::Path,
    sync::mpsc::{self, Sender},
    thread::{self, JoinHandle},
};

use file_rotate::{
    compression::Compression,
    suffix::{AppendTimestamp, FileLimit},
    ContentLimit, FileRotate,
};
use serde::{Deserialize, Serialize};
use tracing::error;
use utoipa::ToSchema;

use crate::{
    db::{nano_id::NanoId, BuildLog},
    docker::{DockerLog, LogType},
    paths::get_log_dir,
    utils::LogError,
};

const LOG_FILE_PREFIX: &str = "log";

#[derive(Serialize, Deserialize, ToSchema)]
pub(crate) enum Level {
    INFO,
    ERROR,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct RequestLog {
    pub(crate) time: i64,
    pub(crate) level: Level,
    pub(crate) deployment: NanoId,
    pub(crate) host: String,
    pub(crate) method: String, // TODO: make enum out of this?
    pub(crate) path: String,
    pub(crate) status: u16,
    // pub(crate) message: String,
}

#[derive(Serialize, ToSchema)]
pub(crate) struct Log {
    pub(crate) time: i64,
    pub(crate) level: Level,
    pub(crate) deployment: String,
    pub(crate) host: Option<String>,
    pub(crate) method: Option<String>, // TODO: make enum out of this?
    pub(crate) path: Option<String>,
    pub(crate) status: Option<u16>,
    pub(crate) message: Option<String>,
}

impl Log {
    pub(crate) fn from_docker(value: DockerLog, deployment: NanoId) -> Self {
        // TODO: try also inferring the level from the log itself
        let level = if value.log_type == LogType::Out {
            Level::INFO
        } else {
            Level::ERROR
        };

        Self {
            level,
            time: value.time,
            deployment: deployment.into(),
            host: None,
            method: None,
            path: None,
            status: None,
            message: Some(value.message),
        }
    }
}

impl From<RequestLog> for Log {
    fn from(value: RequestLog) -> Self {
        Self {
            level: value.level,
            time: value.time,
            deployment: value.deployment.into(),
            host: Some(value.host),
            method: Some(value.method),
            path: Some(value.path),
            status: Some(value.status),
            message: None,
        }
    }
}

impl From<BuildLog> for Log {
    fn from(value: BuildLog) -> Self {
        let level = if value.error == 0 {
            Level::INFO
        } else {
            Level::ERROR
        };

        Self {
            level,
            time: value.timestamp,
            deployment: value.deployment.into(),
            host: None,
            method: None,
            path: None,
            status: None,
            message: Some(value.content),
        }
    }
}

#[derive(Default)]
pub(crate) struct RequestLogger {
    sender: Option<Sender<RequestLog>>,
    join_handle: Option<JoinHandle<()>>,
}

impl Drop for RequestLogger {
    fn drop(&mut self) {
        self.sender.take();
        if let Some(join_handle) = self.join_handle.take() {
            let _ = join_handle.join();
        }
    }
}

impl RequestLogger {
    pub(crate) fn new() -> Self {
        // FIXME: what happens with restarts here?
        let (sender, receiver) = mpsc::channel::<RequestLog>();

        let join_handle = thread::spawn(move || {
            let file_path = get_log_dir().join(LOG_FILE_PREFIX);
            let mut log = FileRotate::new(
                file_path,
                AppendTimestamp::default(FileLimit::MaxFiles(10)),
                ContentLimit::Time(file_rotate::TimeFrequency::Hourly),
                Compression::None,
                None,
            );

            for event in receiver {
                let encoded: Vec<u8> = bincode::serialize(&event).unwrap();
                log.write_all(encoded.as_slice()).ignore_logging();
            }
        });

        Self {
            sender: Some(sender),
            join_handle: Some(join_handle),
        }
    }

    pub(crate) fn log(&self, event: RequestLog) {
        if let Some(sender) = &self.sender {
            sender.send(event).ignore_logging();
        }
    }
}

struct EventIter {
    file: File,
}

impl EventIter {
    fn new(path: &Path) -> io::Result<Self> {
        Ok(Self {
            file: File::open(path)?,
        })
    }
}

impl Iterator for EventIter {
    type Item = Log;
    fn next(&mut self) -> Option<Self::Item> {
        let event: RequestLog = bincode::deserialize_from(&self.file).ok()?;
        Some(event.into())
    }
}

pub(crate) fn read_request_event_logs() -> io::Result<impl Iterator<Item = Log>> {
    // TODO: accept window

    let mut paths: Vec<_> = fs::read_dir(get_log_dir())?
        .filter_map(|entry| Some(entry.ok()?))
        .collect();

    paths.sort_by_key(|path| path.file_name());
    paths.reverse();
    let current_position = paths
        .iter()
        .position(|path| path.file_name() == LOG_FILE_PREFIX);
    if let Some(current_position) = current_position {
        paths.swap(current_position, 0);
    }
    // here paths is ordered like: log, log.20241023T072726, log.20241023T062746, ...
    // i.e. starting from the most recent

    let events = paths
        .into_iter()
        .filter_map(|path| EventIter::new(&path.path()).ok())
        .take(2)
        .flatten();
    Ok(events)
}

// #[cfg(test)]
// mod log_tests {
//     use std::{fs::File, io::Write};

//     use serde::{Deserialize, Serialize};

//     #[derive(Serialize, Deserialize)]
//     struct Test {
//         a: i64,
//         b: String,
//     }

//     fn write(file: &mut File, element: Test) {
//         let encoded: Vec<u8> = bincode::serialize(&element).unwrap();
//         file.write_all(encoded.as_slice());
//     }

//     #[test]
//     fn test_bincode() {
//         let file = File::open("/tmp/bincode-test").unwrap();

//         write(
//             &mut file,
//             Test {
//                 a: 1,
//                 b: "hello1".to_owned(),
//             },
//         );
//         write(
//             &mut file,
//             Test {
//                 a: 2,
//                 b: "hello2".to_owned(),
//             },
//         );

//         // read !!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!

//         let decoded: Option<String> = bincode::deserialize(&encoded[..]).unwrap();
//         assert_eq!(target, decoded);
//     }
// }
