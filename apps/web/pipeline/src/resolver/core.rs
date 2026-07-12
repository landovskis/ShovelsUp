use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddressResolutionDecision {
    Link { project_id: Uuid },
    CreateProject,
    FlagAmbiguous,
}

pub fn decide_address_resolution(
    exact_match: Option<Uuid>,
    other_type_match_count: usize,
) -> AddressResolutionDecision {
    match exact_match {
        Some(project_id) => AddressResolutionDecision::Link { project_id },
        None if other_type_match_count == 0 => AddressResolutionDecision::CreateProject,
        None => AddressResolutionDecision::FlagAmbiguous,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_match_always_wins() {
        let project_id = Uuid::new_v4();
        assert_eq!(
            decide_address_resolution(Some(project_id), 3),
            AddressResolutionDecision::Link { project_id }
        );
    }

    #[test]
    fn no_matches_creates_a_project() {
        assert_eq!(
            decide_address_resolution(None, 0),
            AddressResolutionDecision::CreateProject
        );
    }

    #[test]
    fn different_type_matches_require_review() {
        assert_eq!(
            decide_address_resolution(None, 1),
            AddressResolutionDecision::FlagAmbiguous
        );
    }
}
