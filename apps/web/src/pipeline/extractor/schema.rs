use serde::Deserialize;

/// The shape the LLM is instructed to return (see `prompts::en::SYSTEM_PROMPT`
/// and the matching `output_config.format` JSON Schema in `llm::anthropic`).
/// `has_mention` lets the model say "this chunk describes no construction
/// project" without fighting the schema — every other field is nullable.
#[derive(Debug, Clone, Deserialize)]
pub struct RawExtraction {
    pub has_mention: bool,
    /// The LLM's own physical-work classification — never trusted directly;
    /// see `validator::validate_physical_work`, which overrides it
    /// deterministically per RULE-001.
    pub physical_work: bool,
    pub project_name: Option<String>,
    pub civic_address: Option<String>,
    pub project_type: Option<String>,
    pub scale_units: Option<i32>,
    pub scale_gfa_sqm: Option<f64>,
    pub scale_storeys: Option<i32>,
    pub approval_status_raw: Option<String>,
    /// An explicit application/permit/file reference number (e.g.
    /// "Application No. 2026-045") — added for REQ-005's cross-reference
    /// matcher (RULE-003), which needs a concrete signal to match mentions
    /// of the same project across separate agenda items. `serde(default)`
    /// so hand-written test JSON fixtures predating this field don't need
    /// updating — the real API always includes it (the JSON Schema below
    /// still marks it required for structured-output responses).
    #[serde(default)]
    pub reference_number: Option<String>,
}

/// A validated extraction ready to persist as a `project_mentions` row.
#[derive(Debug, Clone, PartialEq)]
pub struct ExtractionResult {
    pub physical_work: bool,
    pub project_name: Option<String>,
    pub civic_address: Option<String>,
    pub project_type: Option<String>,
    pub scale_units: Option<i32>,
    pub scale_gfa_sqm: Option<f64>,
    pub scale_storeys: Option<i32>,
    pub approval_status_raw: Option<String>,
    pub reference_number: Option<String>,
}

/// JSON Schema sent as `output_config.format` on the Anthropic Messages API
/// request — structured outputs guarantee the response validates against
/// this exactly, matching `RawExtraction`'s fields (IMP-REQ-003-02).
pub fn extraction_json_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "has_mention": {
                "type": "boolean",
                "description": "true if this chunk describes any construction or development project"
            },
            "physical_work": {
                "type": "boolean",
                "description": "true only if this describes an actual physical construction/demolition/renovation project, not a purely administrative, rezoning-only, or procedural matter (RULE-001)"
            },
            "project_name": { "anyOf": [{ "type": "string" }, { "type": "null" }] },
            "civic_address": { "anyOf": [{ "type": "string" }, { "type": "null" }] },
            "project_type": {
                "anyOf": [{ "type": "string" }, { "type": "null" }],
                "description": "e.g. residential, commercial, mixed-use, institutional, infrastructure"
            },
            "scale_units": { "anyOf": [{ "type": "integer" }, { "type": "null" }] },
            "scale_gfa_sqm": { "anyOf": [{ "type": "number" }, { "type": "null" }] },
            "scale_storeys": { "anyOf": [{ "type": "integer" }, { "type": "null" }] },
            "approval_status_raw": {
                "anyOf": [{ "type": "string" }, { "type": "null" }],
                "description": "the approval status exactly as it appears in the source text, unmodified"
            },
            "reference_number": {
                "anyOf": [{ "type": "string" }, { "type": "null" }],
                "description": "an explicit application, permit, or file reference number if one is stated (e.g. \"Application No. 2026-045\"), otherwise null"
            }
        },
        "required": [
            "has_mention", "physical_work", "project_name", "civic_address",
            "project_type", "scale_units", "scale_gfa_sqm", "scale_storeys",
            "approval_status_raw", "reference_number"
        ],
        "additionalProperties": false
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The real API always includes reference_number (it's in the JSON
    /// Schema's `required` list), but hand-written test fixtures predating
    /// this field omit it — `serde(default)` must let those still parse.
    #[test]
    fn raw_extraction_deserializes_without_reference_number_field() {
        let json = r#"{"has_mention":true,"physical_work":true,"project_name":null,"civic_address":"123 Main St","project_type":"residential","scale_units":10,"scale_gfa_sqm":null,"scale_storeys":null,"approval_status_raw":"Approved."}"#;
        let raw: RawExtraction = serde_json::from_str(json).expect("should deserialize without reference_number");
        assert_eq!(raw.reference_number, None);
    }

    #[test]
    fn raw_extraction_deserializes_with_reference_number_present() {
        let json = r#"{"has_mention":true,"physical_work":true,"project_name":null,"civic_address":"123 Main St","project_type":"residential","scale_units":10,"scale_gfa_sqm":null,"scale_storeys":null,"approval_status_raw":"Approved.","reference_number":"Application No. 2026-045"}"#;
        let raw: RawExtraction = serde_json::from_str(json).unwrap();
        assert_eq!(raw.reference_number.as_deref(), Some("Application No. 2026-045"));
    }
}
