//! Facts about a lane, from git, the queue and the terminals (M7 slice 5).
//!
//! Pure functions over plain inputs. The GUI gathers the inputs in a
//! background task; nothing here runs git, reads a file or looks at terminal
//! output. A fact is only what the inputs say: "waiting for you" is decided by
//! the conflict count, the decisions awaiting a verdict and an explicit
//! attention request from a lane terminal, never by inferring from output.

use rvw_core::types::Comment;

use crate::queue;

/// What git says about a lane's worktree.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GitFacts {
    /// files with uncommitted changes, staged or not, tracked or new
    pub changed_files: usize,
    /// commits the branch has that its upstream lacks
    pub ahead: usize,
    /// commits the upstream has that the branch lacks
    pub behind: usize,
    /// whether the branch tracks an upstream; without one, ahead and behind are 0
    pub has_upstream: bool,
    /// a merge is stopped part way (`MERGE_HEAD` exists)
    pub merging: bool,
    /// a rebase is stopped part way
    pub rebasing: bool,
    /// files with unresolved conflict markers or an unmerged index entry
    pub conflicted_files: usize,
}

/// The review queue as it stands for a lane.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct QueueFacts {
    /// comments the agent has not finished: pending or pulled
    pub open: usize,
    /// comments an agent decided after a handoff that nobody has judged yet
    pub awaiting_verdict: usize,
}

/// The terminals a lane has open.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TerminalFacts {
    /// terminals whose program is still running
    pub running: usize,
    /// terminals whose program exited with a failure status
    pub exited_failed: usize,
    /// a lane terminal asked for attention (OSC 9 or 777) and is not on screen
    pub asked_attention_hidden: bool,
}

/// What the reviewer has not looked at yet.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ReadFacts {
    /// changed files the reviewer has not opened since they last changed
    pub unread_files: usize,
}

/// Every fact about one lane.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LaneFacts {
    pub git: GitFacts,
    pub queue: QueueFacts,
    pub terminal: TerminalFacts,
    pub read: ReadFacts,
}

/// The queue facts of a workspace's comments, from the queue's own predicates.
pub fn queue_facts(comments: &[Comment]) -> QueueFacts {
    QueueFacts {
        open: comments.iter().filter(|c| c.is_open()).count(),
        awaiting_verdict: queue::waiting_on_you(comments),
    }
}

/// Whether the lane needs the reviewer now: a conflict, a decision awaiting a
/// verdict, or a terminal that asked for attention and is out of sight.
/// Changed files, commits, unread files and running processes never trigger it.
#[must_use]
pub fn waiting_for_you(f: &LaneFacts) -> bool {
    f.git.conflicted_files > 0 || f.queue.awaiting_verdict > 0 || f.terminal.asked_attention_hidden
}

/// The passive facts of a lane as fixed sentences, in a fixed order. Only the
/// facts that hold appear; there is no advice, no progress and no next step.
#[must_use]
pub fn statements(f: &LaneFacts) -> Vec<String> {
    let mut out = Vec::new();
    if f.git.changed_files > 0 {
        out.push(plural(f.git.changed_files, "file changed", "files changed"));
    }
    if f.git.ahead > 0 || f.git.behind > 0 {
        let mut arrows = Vec::new();
        if f.git.ahead > 0 {
            arrows.push(format!("↑{}", f.git.ahead));
        }
        if f.git.behind > 0 {
            arrows.push(format!("↓{}", f.git.behind));
        }
        out.push(arrows.join(" "));
    }
    if f.git.merging {
        out.push("merge in progress".to_string());
    }
    if f.git.rebasing {
        out.push("rebase in progress".to_string());
    }
    if f.git.conflicted_files > 0 {
        out.push(plural(f.git.conflicted_files, "conflict", "conflicts"));
    }
    if f.queue.open > 0 {
        out.push(plural(f.queue.open, "comment open", "comments open"));
    }
    if f.queue.awaiting_verdict > 0 {
        out.push(plural(
            f.queue.awaiting_verdict,
            "decision awaiting your verdict",
            "decisions awaiting your verdict",
        ));
    }
    if f.terminal.running > 0 {
        out.push(plural(
            f.terminal.running,
            "process running",
            "processes running",
        ));
    }
    if f.terminal.exited_failed > 0 {
        out.push(plural(
            f.terminal.exited_failed,
            "process failed",
            "processes failed",
        ));
    }
    if f.read.unread_files > 0 {
        out.push(plural(f.read.unread_files, "file unread", "files unread"));
    }
    out
}

/// A count and its noun, singular for exactly one.
fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rvw_core::types::{Actor, Status, Verdict, VerdictKind};

    fn comment(id: &str, status: Status) -> Comment {
        Comment {
            points_at: None,
            reopens: None,
            verdict: None,
            id: id.into(),
            status,
            workspace: "/ws".into(),
            lane: None,
            author: None,
            file: "src/a.rs".into(),
            path: "/ws/src/a.rs".into(),
            start_line: 4,
            end_line: 4,
            filetype: "rust".into(),
            code: "let x = 1;".into(),
            comment: "fix this".into(),
            created_at: "2026-01-01T00:00:00Z".into(),
            pulled_at: None,
            resolved_at: None,
            resolved_by: None,
            resolution_note: None,
            file_version: String::new(),
            resolved_file_version: String::new(),
            review_id: String::new(),
            edited_at: String::new(),
        }
    }

    /// A comment an agent decided after a handoff, awaiting the reviewer's verdict.
    fn awaiting(id: &str) -> Comment {
        let mut c = comment(id, Status::Done);
        c.pulled_at = Some("2026-01-02T00:00:00Z".into());
        c.resolved_by = Some("claude".into());
        c
    }

    fn lane(edit: impl FnOnce(&mut LaneFacts)) -> LaneFacts {
        let mut f = LaneFacts::default();
        edit(&mut f);
        f
    }

    #[test]
    fn queue_facts_counts_open_and_awaiting() {
        let mut judged = awaiting("r5");
        judged.verdict = Some(Verdict {
            kind: VerdictKind::Accepted,
            by: "ada".into(),
            actor: Actor::Human,
            at: "2026-01-03T00:00:00Z".into(),
            note: None,
            followup: None,
        });
        let cases: Vec<(&str, Vec<Comment>, QueueFacts)> = vec![
            ("none", vec![], QueueFacts::default()),
            (
                "pending and pulled are open",
                vec![
                    comment("r1", Status::Pending),
                    comment("r2", Status::Pulled),
                ],
                QueueFacts {
                    open: 2,
                    awaiting_verdict: 0,
                },
            ),
            (
                "done and rejected are not open",
                vec![comment("r1", Status::Done), comment("r2", Status::Rejected)],
                QueueFacts {
                    open: 0,
                    awaiting_verdict: 0,
                },
            ),
            (
                "a handed-off decision awaits a verdict, and is not open",
                vec![awaiting("r1")],
                QueueFacts {
                    open: 0,
                    awaiting_verdict: 1,
                },
            ),
            (
                "a judged decision no longer awaits",
                vec![judged],
                QueueFacts {
                    open: 0,
                    awaiting_verdict: 0,
                },
            ),
        ];
        for (name, comments, want) in cases {
            assert_eq!(queue_facts(&comments), want, "{name}");
        }
    }

    #[test]
    fn changed_files_wording() {
        let cases = [
            (0, vec![]),
            (1, vec!["1 file changed"]),
            (3, vec!["3 files changed"]),
        ];
        for (n, want) in cases {
            let f = lane(|f| f.git.changed_files = n);
            assert_eq!(statements(&f), want, "changed_files {n}");
        }
    }

    #[test]
    fn ahead_behind_wording() {
        let cases = [
            ((0, 0), vec![]),
            ((2, 0), vec!["↑2"]),
            ((0, 1), vec!["↓1"]),
            ((2, 1), vec!["↑2 ↓1"]),
        ];
        for ((ahead, behind), want) in cases {
            let f = lane(|f| {
                f.git.ahead = ahead;
                f.git.behind = behind;
            });
            assert_eq!(statements(&f), want, "ahead {ahead} behind {behind}");
        }
    }

    #[test]
    fn merging_and_rebasing_wording() {
        let cases = [
            (false, false, vec![]),
            (true, false, vec!["merge in progress"]),
            (false, true, vec!["rebase in progress"]),
            (true, true, vec!["merge in progress", "rebase in progress"]),
        ];
        for (merging, rebasing, want) in cases {
            let f = lane(|f| {
                f.git.merging = merging;
                f.git.rebasing = rebasing;
            });
            assert_eq!(
                statements(&f),
                want,
                "merging {merging} rebasing {rebasing}"
            );
        }
    }

    #[test]
    fn conflicts_wording() {
        let cases = [
            (0, vec![]),
            (1, vec!["1 conflict"]),
            (2, vec!["2 conflicts"]),
        ];
        for (n, want) in cases {
            let f = lane(|f| f.git.conflicted_files = n);
            assert_eq!(statements(&f), want, "conflicted_files {n}");
        }
    }

    #[test]
    fn open_comments_wording() {
        let cases = [
            (0, vec![]),
            (1, vec!["1 comment open"]),
            (4, vec!["4 comments open"]),
        ];
        for (n, want) in cases {
            let f = lane(|f| f.queue.open = n);
            assert_eq!(statements(&f), want, "open {n}");
        }
    }

    #[test]
    fn awaiting_verdict_wording() {
        let cases = [
            (0, vec![]),
            (1, vec!["1 decision awaiting your verdict"]),
            (2, vec!["2 decisions awaiting your verdict"]),
        ];
        for (n, want) in cases {
            let f = lane(|f| f.queue.awaiting_verdict = n);
            assert_eq!(statements(&f), want, "awaiting_verdict {n}");
        }
    }

    #[test]
    fn running_wording() {
        let cases = [
            (0, vec![]),
            (1, vec!["1 process running"]),
            (2, vec!["2 processes running"]),
        ];
        for (n, want) in cases {
            let f = lane(|f| f.terminal.running = n);
            assert_eq!(statements(&f), want, "running {n}");
        }
    }

    #[test]
    fn failed_wording() {
        let cases = [
            (0, vec![]),
            (1, vec!["1 process failed"]),
            (3, vec!["3 processes failed"]),
        ];
        for (n, want) in cases {
            let f = lane(|f| f.terminal.exited_failed = n);
            assert_eq!(statements(&f), want, "exited_failed {n}");
        }
    }

    #[test]
    fn unread_wording() {
        let cases = [
            (0, vec![]),
            (1, vec!["1 file unread"]),
            (5, vec!["5 files unread"]),
        ];
        for (n, want) in cases {
            let f = lane(|f| f.read.unread_files = n);
            assert_eq!(statements(&f), want, "unread_files {n}");
        }
    }

    #[test]
    fn statements_are_in_a_fixed_order_and_only_for_facts_that_hold() {
        let everything = lane(|f| {
            f.git.changed_files = 3;
            f.git.ahead = 2;
            f.git.behind = 1;
            f.git.merging = true;
            f.git.rebasing = true;
            f.git.conflicted_files = 2;
            f.queue.open = 4;
            f.queue.awaiting_verdict = 1;
            f.terminal.running = 1;
            f.terminal.exited_failed = 1;
            f.read.unread_files = 5;
        });
        assert_eq!(
            statements(&everything),
            vec![
                "3 files changed",
                "↑2 ↓1",
                "merge in progress",
                "rebase in progress",
                "2 conflicts",
                "4 comments open",
                "1 decision awaiting your verdict",
                "1 process running",
                "1 process failed",
                "5 files unread",
            ]
        );
        assert!(statements(&LaneFacts::default()).is_empty());
    }

    #[test]
    fn a_conflict_alone_waits_for_you() {
        assert!(waiting_for_you(&lane(|f| f.git.conflicted_files = 1)));
    }

    #[test]
    fn an_awaiting_verdict_alone_waits_for_you() {
        assert!(waiting_for_you(&lane(|f| f.queue.awaiting_verdict = 1)));
    }

    #[test]
    fn a_hidden_attention_request_alone_waits_for_you() {
        assert!(waiting_for_you(&lane(|f| f
            .terminal
            .asked_attention_hidden =
            true)));
    }

    #[test]
    fn other_facts_alone_do_not_wait_for_you() {
        let cases: Vec<(&str, LaneFacts)> = vec![
            ("none", LaneFacts::default()),
            ("changed files", lane(|f| f.git.changed_files = 3)),
            ("ahead", lane(|f| f.git.ahead = 2)),
            ("behind", lane(|f| f.git.behind = 1)),
            ("merging", lane(|f| f.git.merging = true)),
            ("rebasing", lane(|f| f.git.rebasing = true)),
            ("open comments", lane(|f| f.queue.open = 4)),
            ("unread files", lane(|f| f.read.unread_files = 5)),
            ("running", lane(|f| f.terminal.running = 1)),
            ("failed", lane(|f| f.terminal.exited_failed = 1)),
        ];
        for (name, f) in cases {
            assert!(!waiting_for_you(&f), "{name} alone must not wait for you");
        }
    }
}
