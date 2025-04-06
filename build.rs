use std::env;
use std::process::Command;

fn main() {
    env::set_var("DATABASE_URL", "sqlite:src.db");
    println!("cargo::rustc-env=DATABASE_URL=sqlite:src.db");
    let status = Command::new("cargo")
        .arg("sqlx")
        .arg("database")
        .arg("reset")
        .arg("-y")
        .status()
        .unwrap();
    assert!(status.success());
    println!("cargo:rerun-if-changed=src.db");
    println!("cargo:rerun-if-changed=migrations");
}
