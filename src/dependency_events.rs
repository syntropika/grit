use chrono::{DateTime, FixedOffset};

pub(crate) enum DependencyEvent {
    Mutation(RelationshipMutation),
    Ignored,
    ReconcileRequired,
}

pub(crate) struct RelationshipMutation {
    pub(crate) action: RelationshipAction,
    pub(crate) blocked: IssueReference,
    pub(crate) blocker: IssueReference,
}

pub(crate) enum RelationshipAction {
    Add,
    Remove,
}

pub(crate) struct IssueReference {
    pub(crate) repository: String,
    pub(crate) number: u64,
    pub(crate) state: String,
}

pub(crate) struct RawEvent {
    pub(crate) kind: String,
    pub(crate) created_at: String,
    pub(crate) issue: Option<RawIssueReference>,
    pub(crate) blocked_by: Option<RawIssueReference>,
    pub(crate) blocking: Option<RawIssueReference>,
}

pub(crate) struct RawIssueReference {
    pub(crate) repository: Option<String>,
    pub(crate) number: u64,
    pub(crate) state: String,
}

impl DependencyEvent {
    pub(crate) fn classify(raw: RawEvent) -> Self {
        let relationship = match raw.kind.as_str() {
            "blocked_by_added" => Some((RelationshipAction::Add, raw.issue, raw.blocked_by)),
            "blocked_by_removed" => Some((RelationshipAction::Remove, raw.issue, raw.blocked_by)),
            "blocking_added" => Some((RelationshipAction::Add, raw.blocking, raw.issue)),
            "blocking_removed" => Some((RelationshipAction::Remove, raw.blocking, raw.issue)),
            "transferred" | "deleted" => return Self::ReconcileRequired,
            kind if kind.contains("blocked")
                || kind.contains("blocking")
                || raw.blocked_by.is_some()
                || raw.blocking.is_some() =>
            {
                return Self::ReconcileRequired;
            }
            kind if is_known_non_dependency_event(kind) => return Self::Ignored,
            _ => return Self::ReconcileRequired,
        };
        let Some((action, blocked, blocker)) = relationship else {
            return Self::Ignored;
        };
        let Ok(_) = DateTime::<FixedOffset>::parse_from_rfc3339(&raw.created_at) else {
            return Self::ReconcileRequired;
        };
        let (Some(blocked), Some(blocker)) = (blocked, blocker) else {
            return Self::ReconcileRequired;
        };
        let (Some(blocked), Some(blocker)) = (blocked.complete(), blocker.complete()) else {
            return Self::ReconcileRequired;
        };
        Self::Mutation(RelationshipMutation {
            action,
            blocked,
            blocker,
        })
    }
}

impl RawIssueReference {
    fn complete(self) -> Option<IssueReference> {
        Some(IssueReference {
            repository: self.repository?,
            number: self.number,
            state: self.state,
        })
    }
}

fn is_known_non_dependency_event(kind: &str) -> bool {
    matches!(
        kind,
        "added_to_project"
            | "assigned"
            | "automatic_base_change_failed"
            | "automatic_base_change_succeeded"
            | "base_ref_changed"
            | "closed"
            | "commented"
            | "committed"
            | "connected"
            | "convert_to_draft"
            | "converted_note_to_issue"
            | "converted_to_discussion"
            | "cross-referenced"
            | "demilestoned"
            | "deployed"
            | "deployment_environment_changed"
            | "disconnected"
            | "head_ref_deleted"
            | "head_ref_force_pushed"
            | "head_ref_restored"
            | "issue_type_added"
            | "issue_type_changed"
            | "issue_type_removed"
            | "labeled"
            | "locked"
            | "marked_as_duplicate"
            | "mentioned"
            | "merged"
            | "milestoned"
            | "moved_columns_in_project"
            | "parent_issue_added"
            | "parent_issue_removed"
            | "pinned"
            | "ready_for_review"
            | "referenced"
            | "removed_from_project"
            | "renamed"
            | "reopened"
            | "review_dismissed"
            | "review_request_removed"
            | "review_requested"
            | "reviewed"
            | "sub_issue_added"
            | "sub_issue_removed"
            | "subscribed"
            | "unassigned"
            | "unlabeled"
            | "unlocked"
            | "unmarked_as_duplicate"
            | "unpinned"
            | "unsubscribed"
            | "user_blocked"
    )
}
