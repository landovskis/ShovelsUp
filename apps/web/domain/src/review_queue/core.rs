use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolutionAction {
    Confirm { project_id: Uuid },
    Reject,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CandidateState {
    pub version: i32,
    pub is_open: bool,
    pub mention_id: Option<Uuid>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResolutionPlan {
    pub status: &'static str,
    pub audit_action: &'static str,
    pub project_id: Option<Uuid>,
    pub mention_link: Option<(Uuid, Uuid)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolutionRejection {
    NotOpen,
    VersionConflict,
}

pub fn plan_resolution(
    candidate: CandidateState,
    expected_version: i32,
    action: ResolutionAction,
) -> Result<ResolutionPlan, ResolutionRejection> {
    if !candidate.is_open {
        return Err(ResolutionRejection::NotOpen);
    }
    if candidate.version != expected_version {
        return Err(ResolutionRejection::VersionConflict);
    }

    Ok(match action {
        ResolutionAction::Confirm { project_id } => ResolutionPlan {
            status: "confirmed",
            audit_action: "confirm",
            project_id: Some(project_id),
            mention_link: candidate
                .mention_id
                .map(|mention_id| (mention_id, project_id)),
        },
        ResolutionAction::Reject => ResolutionPlan {
            status: "rejected",
            audit_action: "reject",
            project_id: None,
            mention_link: None,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn confirm_plans_status_audit_and_mention_link() {
        let mention_id = Uuid::new_v4();
        let project_id = Uuid::new_v4();
        let plan = plan_resolution(
            CandidateState {
                version: 3,
                is_open: true,
                mention_id: Some(mention_id),
            },
            3,
            ResolutionAction::Confirm { project_id },
        )
        .unwrap();

        assert_eq!(plan.status, "confirmed");
        assert_eq!(plan.audit_action, "confirm");
        assert_eq!(plan.mention_link, Some((mention_id, project_id)));
    }

    #[test]
    fn reject_never_links_a_mention() {
        let plan = plan_resolution(
            CandidateState {
                version: 1,
                is_open: true,
                mention_id: Some(Uuid::new_v4()),
            },
            1,
            ResolutionAction::Reject,
        )
        .unwrap();

        assert_eq!(plan.status, "rejected");
        assert_eq!(plan.mention_link, None);
    }

    #[test]
    fn validation_precedes_transition_planning() {
        let closed = CandidateState {
            version: 1,
            is_open: false,
            mention_id: None,
        };
        assert_eq!(
            plan_resolution(closed, 1, ResolutionAction::Reject),
            Err(ResolutionRejection::NotOpen)
        );

        let stale = CandidateState {
            version: 2,
            is_open: true,
            mention_id: None,
        };
        assert_eq!(
            plan_resolution(stale, 1, ResolutionAction::Reject),
            Err(ResolutionRejection::VersionConflict)
        );
    }
}
