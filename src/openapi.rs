use std::fs;

use api::server::get_open_api;
use conf::Conf;

mod api;
mod conf;
mod container;
mod db;
mod deployments;
mod docker;
mod docker_bridge;
mod env;
mod github;
mod hooks;
mod label;
mod listener;
mod logging;
mod nixpacks;
mod paths;
mod provider;
mod proxy;
mod sqlite_db;
mod tls;
mod tokens;
mod traces;
mod utils;

fn main() {
    let openapi = get_open_api();
    fs::write("docs/public/openapi.json", openapi.to_json().unwrap()).unwrap();
}
