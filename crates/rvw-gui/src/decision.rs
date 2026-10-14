//! The decision a reviewer gives a submitted review, and the decision string the
//! service takes for it.

/// The reviewer's decision on the review as a whole.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewDecision {
    Comment,
    Approve,
    RequestChanges,
}

impl ReviewDecision {
    /// The three decisions, in the order the dialog shows them.
    pub const ALL: [ReviewDecision; 3] = [
        ReviewDecision::Comment,
        ReviewDecision::Approve,
        ReviewDecision::RequestChanges,
    ];

    /// The decision a dialog opens with: request changes when there are drafts.
    pub fn for_drafts(count: usize) -> Self {
        if count > 0 {
            ReviewDecision::RequestChanges
        } else {
            ReviewDecision::Comment
        }
    }

    /// The decision string the service takes.
    pub fn decision(self) -> &'static str {
        match self {
            ReviewDecision::Comment => "comment",
            ReviewDecision::Approve => "approve",
            ReviewDecision::RequestChanges => "request-changes",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ReviewDecision::Comment => "Comment",
            ReviewDecision::Approve => "Approve",
            ReviewDecision::RequestChanges => "Request changes",
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            ReviewDecision::Comment => "Notes only. The agent still decides on each comment.",
            ReviewDecision::Approve => "Fine to merge. The agent still decides on each comment.",
            ReviewDecision::RequestChanges => "The agent must decide on each comment.",
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            ReviewDecision::Comment => "comment",
            ReviewDecision::Approve => "approve",
            ReviewDecision::RequestChanges => "request-changes",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decision_strings_are_the_service_strings() {
        let decisions: Vec<&str> = ReviewDecision::ALL.iter().map(|v| v.decision()).collect();
        assert_eq!(decisions, ["comment", "approve", "request-changes"]);
    }

    #[test]
    fn dialog_opens_on_request_changes_when_there_are_drafts() {
        assert_eq!(
            ReviewDecision::for_drafts(2),
            ReviewDecision::RequestChanges
        );
        assert_eq!(ReviewDecision::for_drafts(0), ReviewDecision::Comment);
    }
}
