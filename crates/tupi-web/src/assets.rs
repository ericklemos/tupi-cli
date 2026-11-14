//! Assets do frontend embutidos no binário.

use axum::http::header;
use axum::response::{IntoResponse, Response};

const INDEX: &str = include_str!("../assets/index.html");
const STYLES: &str = include_str!("../assets/styles.css");
const APP: &str = include_str!("../assets/app.js");
const GRAPH: &str = include_str!("../assets/graph.js");

fn with_content_type(body: &'static str, content_type: &'static str) -> Response {
    ([(header::CONTENT_TYPE, content_type)], body).into_response()
}

pub(crate) async fn index() -> Response {
    with_content_type(INDEX, "text/html; charset=utf-8")
}

pub(crate) async fn styles() -> Response {
    with_content_type(STYLES, "text/css; charset=utf-8")
}

pub(crate) async fn app() -> Response {
    with_content_type(APP, "application/javascript; charset=utf-8")
}

pub(crate) async fn graph() -> Response {
    with_content_type(GRAPH, "application/javascript; charset=utf-8")
}
