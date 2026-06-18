pub mod agent_secrets;
pub mod import;
pub mod models;
pub mod routes;

use axum::Router;

use crate::state::AppState;

pub fn router() -> Router<AppState> {
    routes::router()
        .merge(agent_secrets::router())
        .nest("/import", import::router())
}
