/// The PRD accepts "at least one" scale indicator (units, GFA, or storeys)
/// rather than requiring all three — a single-indicator fixture must be
/// accepted (TC-REQ-003-2).
pub fn has_scale_indicator(units: Option<i32>, gfa_sqm: Option<f64>, storeys: Option<i32>) -> bool {
    units.is_some() || gfa_sqm.is_some() || storeys.is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// TC-REQ-003-2: single scale-indicator fixture accepted.
    #[test]
    fn accepts_units_only() {
        assert!(has_scale_indicator(Some(24), None, None));
    }

    #[test]
    fn accepts_gfa_only() {
        assert!(has_scale_indicator(None, Some(1200.5), None));
    }

    #[test]
    fn accepts_storeys_only() {
        assert!(has_scale_indicator(None, None, Some(6)));
    }

    #[test]
    fn rejects_all_absent() {
        assert!(!has_scale_indicator(None, None, None));
    }

    #[test]
    fn accepts_all_present() {
        assert!(has_scale_indicator(Some(24), Some(1200.5), Some(6)));
    }
}
