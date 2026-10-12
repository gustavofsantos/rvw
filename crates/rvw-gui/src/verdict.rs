//! The verdict a reviewer gives a submitted review, and the decision string the
//! service takes for it.

/// The reviewer's verdict on the review as a whole.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Comment,
    Approve,
    RequestChanges,
}

impl Verdict {
    /// The three verdicts, in the order the dialog shows them.
    pub const ALL: [Verdict; 3] = [Verdict::Comment, Verdict::Approve, Verdict::RequestChanges];

    /// The verdict a dialog opens with: request changes when there are drafts.
    pub fn for_drafts(count: usize) -> Self {
        if count > 0 {
            Verdict::RequestChanges
        } else {
            Verdict::Comment
        }
    }

    /// The decision string the service takes.
    pub fn decision(self) -> &'static str {
        match self {
            Verdict::Comment => "comment",
            Verdict::Approve => "approve",
            Verdict::RequestChanges => "request-changes",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Verdict::Comment => "Comment",
            Verdict::Approve => "Approve",
            Verdict::RequestChanges => "Request changes",
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            Verdict::Comment => "Notes only. The agent still decides on each comment.",
            Verdict::Approve => "Fine to merge. The agent still decides on each comment.",
            Verdict::RequestChanges => "The agent must decide on each comment.",
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Verdict::Comment => "comment",
            Verdict::Approve => "approve",
            Verdict::RequestChanges => "request-changes",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verdict_decisions_are_the_service_strings() {
        let decisions: Vec<&str> = Verdict::ALL.iter().map(|v| v.decision()).collect();
        assert_eq!(decisions, ["comment", "approve", "request-changes"]);
    }

    #[test]
    fn dialog_opens_on_request_changes_when_there_are_drafts() {
        assert_eq!(Verdict::for_drafts(2), Verdict::RequestChanges);
        assert_eq!(Verdict::for_drafts(0), Verdict::Comment);
    }
}
