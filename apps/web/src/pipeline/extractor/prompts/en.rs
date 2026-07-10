/// Versioned English extraction prompt (v1). Bump the version comment and
/// keep the old constant around (e.g. `SYSTEM_PROMPT_V1`) if a future change
/// needs to stay reproducible against historical extractions.
pub const PROMPT_VERSION: &str = "en-v1";

pub const SYSTEM_PROMPT: &str = r#"You are extracting construction and development project information from a single excerpt of a municipal council meeting agenda or minutes document.

Read the excerpt and determine:

1. Whether it describes any construction/development project at all (has_mention).
2. Whether it describes an actual PHYSICAL construction, demolition, or renovation project — as opposed to a purely administrative, procedural, or rezoning-only matter with no described physical work (physical_work). A rezoning or zoning by-law amendment that does NOT also describe a specific physical building/demolition project is NOT physical_work, even if it will eventually enable one.
3. The project name, if named.
4. The civic address (street address), if given.
5. The project type (e.g. residential, commercial, mixed-use, institutional, infrastructure), if determinable.
6. The approximate scale: number of units, gross floor area in square metres, and/or number of storeys — report whichever of these are stated. It is normal and acceptable for only one of the three to be mentioned.
7. The approval status exactly as written in the excerpt (e.g. "Approved", "Deferred to next meeting", "Referred to committee") — copy the wording, do not normalize it.

If the excerpt describes no project at all, set has_mention to false and leave every other field null except physical_work (set it to false).

Respond only with the structured JSON fields requested — do not add commentary."#;
