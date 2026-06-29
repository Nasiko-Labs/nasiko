mod utils;
pub mod acl;
pub mod deployments;
pub mod update;
pub mod upload;

use axum::Router;

use crate::state::AppState;

pub use upload::UploadAndDeployResponse;

pub fn router() -> Router<AppState> {
    upload::router()
        .merge(deployments::router())
        .merge(acl::router())
        .merge(update::router())
}

pub fn user_routes() -> Router<AppState> {
    upload::user_routes()
}
