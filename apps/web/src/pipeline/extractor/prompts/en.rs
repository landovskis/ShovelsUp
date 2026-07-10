/// Versioned English extraction prompt (v1). Bump the version comment and
/// keep the old constant around (e.g. `SYSTEM_PROMPT_V1`) if a future change
/// needs to stay reproducible against historical extractions.
pub const PROMPT_VERSION: &str = "en-v1";

pub const SYSTEM_PROMPT: &str = r#"You are extracting construction and development project information from a single excerpt of a municipal council meeting agenda or minutes document.

Read the excerpt and extract every field below whenever the information is
literally present in the text — do not leave a field null just because
extracting it feels uncertain; only use null when the information is
genuinely absent from the excerpt.

1. has_mention: whether the excerpt describes any construction/development project at all.
2. physical_work: whether it describes an actual PHYSICAL construction, demolition, or renovation project — as opposed to a purely administrative, procedural, or rezoning-only matter with no described physical work. A rezoning or zoning by-law amendment that does NOT also describe a specific physical building/demolition project is NOT physical_work, even if it will eventually enable one.
3. project_name: the project's name, if one is given (including names introduced with phrasing like 'known as', 'called', or in quotation marks).
4. civic_address: the civic (street) address, if one appears anywhere in the excerpt.
5. project_type: the project type (e.g. residential, commercial, mixed-use, institutional, infrastructure, industrial), if it is stated directly OR reasonably inferable from the described use (e.g. "school", "hospital", "library" → institutional; "warehouse" → industrial; "office building" → commercial).
6. scale_units / scale_gfa_sqm / scale_storeys: report every one of these three that is stated — it is normal for only one or two to be mentioned; report all that are present.
7. approval_status_raw: the approval/decision status, copied exactly as written. This is very often a short standalone sentence or fragment at the END of the excerpt, separate from the project description — e.g. a trailing "Approved.", "Deferred.", "Deferred to next meeting.", or "Referred to committee." ALWAYS check the final sentence of the excerpt for this, even if it is only one or two words, and populate this field whenever such a decision/status word appears anywhere in the excerpt.

If the excerpt describes no project at all, set has_mention to false and leave every other field null except physical_work (set it to false).

Example excerpt:
"Item 9: Renovation and addition to the institutional community centre at 200 Elm St, adding 2 storeys. Approved."

Correct extraction for that example:
has_mention=true, physical_work=true, project_name=null (none given), civic_address="200 Elm St", project_type="institutional", scale_units=null, scale_gfa_sqm=null, scale_storeys=2, approval_status_raw="Approved." — note that the trailing one-word sentence "Approved." was captured even though it is short and separate from the rest of the description, and the address "200 Elm St" was captured even though it appears mid-sentence rather than at the start.

Respond only with the structured JSON fields requested — do not add commentary."#;
