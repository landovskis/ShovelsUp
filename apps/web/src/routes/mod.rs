use axum::{extract::State, http::StatusCode, response::Html};
use minijinja::context;

use crate::AppState;

pub async fn index(State(state): State<AppState>) -> Result<Html<String>, StatusCode> {
    let tmpl = state
        .env
        .get_template("index.html")
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let html = tmpl
        .render(context! { title => "ShovelsUp" })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Html(html))
}
