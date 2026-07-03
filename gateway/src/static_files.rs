use axum::{
    body::Body,
    extract::Request,
    http::{StatusCode, header},
    response::{IntoResponse, Redirect, Response},
};
use rust_embed::Embed;

#[derive(Embed)]
#[folder = "../ui/oss/"]
struct OssAssets;

#[derive(Embed)]
#[folder = "../ui/common/"]
#[prefix = "common/"]
struct CommonAssets;

fn is_public_asset(path: &str) -> bool {
    path == "login.html"
        || path.starts_with("common/")
        || path.ends_with(".css")
        || path.ends_with(".woff2")
        || path.ends_with(".woff")
}

fn has_access_token(req: &Request<Body>) -> bool {
    req.headers()
        .get(header::COOKIE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|cookies| {
            cookies.split(';').any(|c| {
                let c = c.trim();
                c.starts_with("access_token=") && c.len() > "access_token=".len()
            })
        })
}

pub async fn static_handler(req: Request<Body>) -> Response {
    let path = req.uri().path().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };

    if !is_public_asset(path) && !has_access_token(&req) {
        return Redirect::to("/login.html").into_response();
    }

    if let Some(file) = OssAssets::get(path).or_else(|| CommonAssets::get(path)) {
        let mime = mime_guess::from_path(path).first_or_octet_stream();
        return ([(header::CONTENT_TYPE, mime.as_ref())], file.data).into_response();
    }

    StatusCode::NOT_FOUND.into_response()
}
