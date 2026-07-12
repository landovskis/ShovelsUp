use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    response::Html,
};
use minijinja::context;

use crate::AppState;

pub mod admin;
pub mod projects;
pub mod review_queue;
pub mod search;

pub(crate) fn detect_lang(headers: &HeaderMap) -> &'static str {
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

    let copy = match lang {
        "fr" => HomeCopy {
            eyebrow: "Les chantiers de Montréal, au grand jour",
            heading_start: "Sachez ce qui se construit",
            heading_highlight: "près de chez vous.",
            tagline: "Suivez les permis, les décisions du conseil et les projets qui transforment votre quartier — sans fouiller dans les dossiers municipaux.",
            search_label: "Trouver un projet",
            search_placeholder: "Adresse ou arrondissement",
            search_button: "Rechercher",
            search_note: "Données municipales réunies en un seul endroit.",
            visual_label: "Aperçu d’un dossier de permis sur une carte de Montréal",
            sample_type: "Transformation",
            sample_status: "À l’étude",
            sample_number: "DOSSIER 3003421578-25",
            sample_address: "4128, rue De Bullion",
            sample_borough_label: "Arrondissement",
            sample_borough: "Le Plateau",
            sample_updated_label: "Mise à jour",
            sample_updated: "Aujourd’hui",
            decision_label: "Dernière décision",
            decision_value: "Avis favorable",
            coverage_label: "19 arrondissements suivis",
            nav_permits: "Permis",
            nav_council: "Conseil",
        },
        _ => HomeCopy {
            eyebrow: "Montreal construction, out in the open",
            heading_start: "Know what’s being built",
            heading_highlight: "next door.",
            tagline: "Follow permits, council decisions, and the projects reshaping your neighbourhood—without digging through city records.",
            search_label: "Find a project",
            search_placeholder: "Address or borough",
            search_button: "Search",
            search_note: "Municipal data, gathered in one clear place.",
            visual_label: "Preview of a permit record on a Montreal map",
            sample_type: "Renovation",
            sample_status: "Under review",
            sample_number: "FILE 3003421578-25",
            sample_address: "4128 De Bullion Street",
            sample_borough_label: "Borough",
            sample_borough: "Le Plateau",
            sample_updated_label: "Updated",
            sample_updated: "Today",
            decision_label: "Latest decision",
            decision_value: "Favourable notice",
            coverage_label: "Tracking 19 boroughs",
            nav_permits: "Permits",
            nav_council: "Council",
        },
    };

    let tmpl = state
        .env
        .get_template("index.html")
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let html = tmpl
        .render(context! {
            title => "ShovelsUp",
            lang => lang,
            eyebrow => copy.eyebrow,
            heading_start => copy.heading_start,
            heading_highlight => copy.heading_highlight,
            tagline => copy.tagline,
            search_label => copy.search_label,
            search_placeholder => copy.search_placeholder,
            search_button => copy.search_button,
            search_note => copy.search_note,
            visual_label => copy.visual_label,
            sample_type => copy.sample_type,
            sample_status => copy.sample_status,
            sample_number => copy.sample_number,
            sample_address => copy.sample_address,
            sample_borough_label => copy.sample_borough_label,
            sample_borough => copy.sample_borough,
            sample_updated_label => copy.sample_updated_label,
            sample_updated => copy.sample_updated,
            decision_label => copy.decision_label,
            decision_value => copy.decision_value,
            coverage_label => copy.coverage_label,
            nav_permits => copy.nav_permits,
            nav_council => copy.nav_council,
        })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Html(html))
}

struct HomeCopy {
    eyebrow: &'static str,
    heading_start: &'static str,
    heading_highlight: &'static str,
    tagline: &'static str,
    search_label: &'static str,
    search_placeholder: &'static str,
    search_button: &'static str,
    search_note: &'static str,
    visual_label: &'static str,
    sample_type: &'static str,
    sample_status: &'static str,
    sample_number: &'static str,
    sample_address: &'static str,
    sample_borough_label: &'static str,
    sample_borough: &'static str,
    sample_updated_label: &'static str,
    sample_updated: &'static str,
    decision_label: &'static str,
    decision_value: &'static str,
    coverage_label: &'static str,
    nav_permits: &'static str,
    nav_council: &'static str,
}
