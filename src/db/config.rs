use serde::{Deserialize, Serialize};

#[derive(sqlx::Type, Deserialize, Serialize, Clone, Copy, Debug, PartialEq)]
#[serde(rename_all = "lowercase")]
#[sqlx(rename_all = "lowercase")]
pub(crate) enum Visibility {
    Standard,
    Public,
    Private,
}

#[derive(sqlx::Type, Deserialize, Serialize, Clone, Copy, Debug)]
#[sqlx(rename_all = "lowercase")]
#[derive(PartialEq)]
pub(crate) enum BuildBackend {
    Dockerfile,
    Nixpacks,
}
