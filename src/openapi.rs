use std::fs;

use api::server::get_open_api;

mod alphabet;
mod api;
mod conf;
mod container;
mod db;
mod deployment_hooks;
mod deployments;
mod docker;
mod env;
mod github;
mod label;
mod listener;
mod logging;
mod paths;
mod sqlite_db;
mod time;
mod tls;

fn main() {
    let openapi = get_open_api();
    fs::write("/tmp/openapi.json", openapi.to_json().unwrap()).unwrap();
}
