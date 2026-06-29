pub mod routes;

use axum::Router;

use crate::state::AppState;

pub fn router() -> Router<AppState> {
    routes::router()
}

pub fn user_routes() -> Router<AppState> {
    routes::user_routes()
}