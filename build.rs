use sqlx::migrate::Migrator;
use sqlx::SqlitePool;
use std::env;

const DATABASE_PATH: &str = "src.db";

#[tokio::main]
async fn main() {
    let database_url = format!("sqlite:{DATABASE_PATH}");
    env::set_var("DATABASE_URL", "sqlite:src.db");
    println!("cargo::rustc-env=DATABASE_URL={database_url}");

    let _ = std::fs::remove_file(DATABASE_PATH);
    std::fs::File::create(DATABASE_PATH).unwrap();
    let pool = SqlitePool::connect(&database_url).await.unwrap();
    let migrator = Migrator::new(std::path::Path::new("./migrations"))
        .await
        .unwrap();
    migrator.run(&pool).await.unwrap();

    println!("cargo:rerun-if-changed=src.db");
    println!("cargo:rerun-if-changed=migrations");
}
