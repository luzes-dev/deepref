use deepref_domain::{
    Actor, ProjectId, ReportId, StudyDesign, StudyDesignContext, StudyId, StudyReportRole,
    StudyTitle,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateStudy {
    pub project_id: ProjectId,
    pub study_id: StudyId,
    pub title: StudyTitle,
    pub actor: Actor,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenameStudy {
    pub project_id: ProjectId,
    pub study_id: StudyId,
    pub title: StudyTitle,
    pub expected_revision: u64,
    pub actor: Actor,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssignReportToStudy {
    pub project_id: ProjectId,
    pub study_id: StudyId,
    pub report_id: ReportId,
    pub role: StudyReportRole,
    pub expected_revision: u64,
    pub expected_previous_study_revision: Option<u64>,
    pub actor: Actor,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoveReportFromStudy {
    pub project_id: ProjectId,
    pub study_id: StudyId,
    pub report_id: ReportId,
    pub expected_revision: u64,
    pub actor: Actor,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassifyStudy {
    pub project_id: ProjectId,
    pub study_id: StudyId,
    pub design: StudyDesign,
    pub context: StudyDesignContext,
    pub expected_revision: u64,
    pub actor: Actor,
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    #[test]
    fn commands_carry_typed_scope_and_revision() {
        let command = ClassifyStudy {
            project_id: Uuid::new_v4().into(),
            study_id: Uuid::new_v4().into(),
            design: StudyDesign::Rct,
            context: StudyDesignContext::default(),
            expected_revision: 4,
            actor: Actor::new(deepref_domain::ActorKind::User, "reviewer").unwrap(),
        };
        assert_eq!(command.expected_revision, 4);
    }
}
