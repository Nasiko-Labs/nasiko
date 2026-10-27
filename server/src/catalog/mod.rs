pub mod agent_secrets;
pub mod discovered;
pub mod import;
pub mod models;
pub mod routes;
pub mod skills;

use axum::Router;

use crate::state::AppState;

pub fn router() -> Router<AppState> {
    routes::router()
        .merge(agent_secrets::router())
        .merge(discovered::router())
        .nest("/import", import::router())
}
