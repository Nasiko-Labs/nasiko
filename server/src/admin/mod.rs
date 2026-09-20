pub mod routes;

use axum::Router;

use crate::state::AppState;

pub fn router() -> Router<AppState> {
    routes::router()
}

pub fn degradable_router() -> Router<AppState> {
    routes::degradable_router()
}
