use axum::{extract::State, http::{HeaderMap, StatusCode}, response::Html};
use minijinja::context;

use crate::AppState;

fn detect_lang(headers: &HeaderMap) -> &'static str {
    let accept = headers
        .get("accept-language")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    for part in accept.split(',') {
        let tag = part.split(';').next().unwrap_or("").trim();
        if tag.starts_with("fr") {
            return "fr";
        }
        if tag.starts_with("en") {
            return "en";
        }
    }
    "en"
}

pub async fn index(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Html<String>, StatusCode> {
    let lang = detect_lang(&headers);

    let (tagline, nav_permits, nav_council) = match lang {
        "fr" => (
            "Suivi des permis de construction dans votre quartier.",
            "Permis",
            "Conseil",
        ),
        _ => (
            "Construction permit tracking for your neighbourhood.",
            "Permits",
            "Council",
        ),
    };

    let tmpl = state
        .env
        .get_template("index.html")
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let html = tmpl
        .render(context! {
            title => "ShovelsUp",
            lang => lang,
            tagline => tagline,
            nav_permits => nav_permits,
            nav_council => nav_council,
        })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Html(html))
}
