//! The key strip: thirteen keys in fixed places and the light each one shows.

/// The part of the strip a key sits in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Group {
    Move,
    Review,
    Git,
    Find,
}

/// The thirteen keys, in strip order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Move,
    Open,
    OpenFile,
    Comment,
    Add,
    Submit,
    Accept,
    Reopen,
    Stage,
    Commit,
    Undo,
    FindFile,
    Actions,
}

/// What a key shows: not available here, available, or needed now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Light {
    Off,
    Available,
    Needed,
}

/// The screen the user is looking at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Diff,
    Outcome,
    Queue,
    Files,
    Search,
    Changes,
    History,
}

impl Key {
    /// Every key, in strip order.
    pub const ALL: [Key; 13] = [
        Key::Move,
        Key::Open,
        Key::OpenFile,
        Key::Comment,
        Key::Add,
        Key::Submit,
        Key::Accept,
        Key::Reopen,
        Key::Stage,
        Key::Commit,
        Key::Undo,
        Key::FindFile,
        Key::Actions,
    ];

    /// The key as shown on its cap.
    pub fn glyph(self) -> &'static str {
        match self {
            Key::Move => "J K",
            Key::Open => "↵",
            Key::OpenFile => "O",
            Key::Comment => "C",
            Key::Add => "⌘↵",
            Key::Submit => "⇧⌘R",
            Key::Accept => "A",
            Key::Reopen => "R",
            Key::Stage => "S",
            Key::Commit => "⇧⌘C",
            Key::Undo => "⌘Z",
            Key::FindFile => "⌘P",
            Key::Actions => "⌘K",
        }
    }

    /// The action name shown beside the cap.
    pub fn label(self) -> &'static str {
        match self {
            Key::Move => "Move",
            Key::Open => "Open",
            Key::OpenFile => "Open file",
            Key::Comment => "Comment",
            Key::Add => "Add",
            Key::Submit => "Submit",
            Key::Accept => "Accept",
            Key::Reopen => "Reopen",
            Key::Stage => "Stage",
            Key::Commit => "Commit",
            Key::Undo => "Undo",
            Key::FindFile => "Find file",
            Key::Actions => "Actions",
        }
    }

    /// The group the key sits in.
    pub fn group(self) -> Group {
        match self {
            Key::Move | Key::Open | Key::OpenFile => Group::Move,
            Key::Comment | Key::Add | Key::Submit | Key::Accept | Key::Reopen => Group::Review,
            Key::Stage | Key::Commit | Key::Undo => Group::Git,
            Key::FindFile | Key::Actions => Group::Find,
        }
    }
}

/// The facts the lights are decided from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)] // independent facts, one per light rule
pub struct Context {
    pub screen: Screen,
    /// A comment composer is open.
    pub composing: bool,
    /// Comments in the user's unsubmitted review.
    pub draft_count: usize,
    /// The selected comment is done by the agent and awaits a verdict.
    pub comment_done: bool,
    /// Files with staged changes.
    pub staged_count: usize,
    /// There is an action to undo.
    pub can_undo: bool,
    /// The screen has a list the user can move through with J and K.
    pub can_move: bool,
    /// The app can open the file in the user's editor.
    pub can_open_file: bool,
}

/// The screens that show a file the user can open in an editor.
fn shows_file(screen: Screen) -> bool {
    matches!(
        screen,
        Screen::Diff | Screen::Outcome | Screen::Files | Screen::Changes
    )
}

/// The lights of all thirteen keys, indexed like [`Key::ALL`].
pub fn lights(ctx: &Context) -> [Light; 13] {
    Key::ALL.map(|key| light(ctx, key))
}

/// The light of one key in this context.
pub fn light(ctx: &Context, key: Key) -> Light {
    let on_diff_or_files = matches!(ctx.screen, Screen::Diff | Screen::Files);
    let outcome_done = ctx.screen == Screen::Outcome && ctx.comment_done;
    match key {
        Key::FindFile | Key::Actions => Light::Available,
        Key::Move => {
            if ctx.can_move && ctx.screen != Screen::Queue {
                Light::Available
            } else {
                Light::Off
            }
        }
        Key::Open => match ctx.screen {
            Screen::Search => Light::Needed,
            Screen::Queue | Screen::Files | Screen::History => Light::Available,
            _ => Light::Off,
        },
        Key::OpenFile => {
            if ctx.can_open_file && shows_file(ctx.screen) {
                Light::Available
            } else {
                Light::Off
            }
        }
        Key::Comment => {
            if on_diff_or_files {
                Light::Available
            } else {
                Light::Off
            }
        }
        Key::Add => {
            if ctx.composing && on_diff_or_files {
                Light::Needed
            } else {
                Light::Off
            }
        }
        Key::Submit => {
            if ctx.draft_count > 0 && on_diff_or_files {
                Light::Available
            } else {
                Light::Off
            }
        }
        Key::Accept => {
            if outcome_done {
                Light::Needed
            } else {
                Light::Off
            }
        }
        Key::Reopen => {
            if outcome_done {
                Light::Available
            } else {
                Light::Off
            }
        }
        Key::Stage => {
            if ctx.screen == Screen::Changes {
                Light::Available
            } else {
                Light::Off
            }
        }
        Key::Commit => {
            if ctx.screen == Screen::Changes && ctx.staged_count > 0 {
                Light::Needed
            } else {
                Light::Off
            }
        }
        Key::Undo => {
            if ctx.can_undo {
                Light::Available
            } else {
                Light::Off
            }
        }
    }
}

/// What unlocks a key that is Off here, or `None` when the key is not Off or
/// has nothing to say.
pub fn hint(ctx: &Context, key: Key) -> Option<&'static str> {
    if light(ctx, key) != Light::Off {
        return None;
    }
    match key {
        Key::Move => Some("Nothing to move through here"),
        Key::Open => Some("Select an item first"),
        Key::OpenFile => Some(if shows_file(ctx.screen) {
            "Opening a file in your editor is not available yet"
        } else {
            "Open a file in the diff or the tree"
        }),
        Key::Comment => Some("Comments are written on a diff or a file"),
        Key::Add => Some("Start a comment first"),
        Key::Submit => Some("Add a comment to your review first"),
        Key::Accept | Key::Reopen => Some("Select a comment the agent has done"),
        Key::Stage => Some("Stage changes on the Changes screen"),
        Key::Commit => Some("Stage something to commit"),
        Key::Undo => Some("Nothing to undo"),
        Key::FindFile | Key::Actions => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCREENS: [Screen; 7] = [
        Screen::Diff,
        Screen::Outcome,
        Screen::Queue,
        Screen::Files,
        Screen::Search,
        Screen::Changes,
        Screen::History,
    ];

    /// A context on `screen` where every capability is on and nothing else is.
    fn ctx(screen: Screen) -> Context {
        Context {
            screen,
            composing: false,
            draft_count: 0,
            comment_done: false,
            staged_count: 0,
            can_undo: false,
            can_move: true,
            can_open_file: true,
        }
    }

    /// Every combination of the context fields, with counts 0 and 1.
    fn all_contexts() -> Vec<Context> {
        let mut out = Vec::new();
        for screen in SCREENS {
            for composing in [false, true] {
                for comment_done in [false, true] {
                    for can_undo in [false, true] {
                        for draft_count in [0, 1] {
                            for staged_count in [0, 1] {
                                for can_move in [false, true] {
                                    for can_open_file in [false, true] {
                                        out.push(Context {
                                            screen,
                                            composing,
                                            draft_count,
                                            comment_done,
                                            staged_count,
                                            can_undo,
                                            can_move,
                                            can_open_file,
                                        });
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        out
    }

    #[test]
    fn move_is_available_on_every_screen_but_queue() {
        for screen in SCREENS {
            let expected = if screen == Screen::Queue {
                Light::Off
            } else {
                Light::Available
            };
            assert_eq!(light(&ctx(screen), Key::Move), expected, "{screen:?}");
        }
    }

    #[test]
    fn move_is_off_when_the_screen_cannot_move() {
        let c = Context {
            can_move: false,
            ..ctx(Screen::Diff)
        };
        assert_eq!(light(&c, Key::Move), Light::Off);
    }

    #[test]
    fn move_hint_explains_the_queue_has_nothing_to_move_through() {
        assert_eq!(
            hint(&ctx(Screen::Queue), Key::Move),
            Some("Nothing to move through here")
        );
        assert_eq!(hint(&ctx(Screen::Diff), Key::Move), None);
    }

    #[test]
    fn open_file_needs_the_app_to_be_able_to_open_files() {
        for screen in SCREENS {
            let c = Context {
                can_open_file: false,
                ..ctx(screen)
            };
            assert_eq!(light(&c, Key::OpenFile), Light::Off, "{screen:?}");
        }
    }

    #[test]
    fn open_file_hint_says_opening_is_not_available_yet_on_file_screens() {
        let c = Context {
            can_open_file: false,
            ..ctx(Screen::Diff)
        };
        assert_eq!(
            hint(&c, Key::OpenFile),
            Some("Opening a file in your editor is not available yet")
        );
        let queue = Context {
            can_open_file: false,
            ..ctx(Screen::Queue)
        };
        assert_eq!(
            hint(&queue, Key::OpenFile),
            Some("Open a file in the diff or the tree")
        );
    }

    #[test]
    fn open_is_needed_on_search_available_on_queue_files_history_off_elsewhere() {
        assert_eq!(light(&ctx(Screen::Search), Key::Open), Light::Needed);
        for screen in [Screen::Queue, Screen::Files, Screen::History] {
            assert_eq!(
                light(&ctx(screen), Key::Open),
                Light::Available,
                "{screen:?}"
            );
        }
        for screen in [Screen::Diff, Screen::Outcome, Screen::Changes] {
            assert_eq!(light(&ctx(screen), Key::Open), Light::Off, "{screen:?}");
        }
    }

    #[test]
    fn open_file_is_available_on_diff_outcome_files_changes_off_elsewhere() {
        for screen in [
            Screen::Diff,
            Screen::Outcome,
            Screen::Files,
            Screen::Changes,
        ] {
            assert_eq!(
                light(&ctx(screen), Key::OpenFile),
                Light::Available,
                "{screen:?}"
            );
        }
        for screen in [Screen::Queue, Screen::Search, Screen::History] {
            assert_eq!(light(&ctx(screen), Key::OpenFile), Light::Off, "{screen:?}");
        }
    }

    #[test]
    fn comment_is_available_on_diff_and_files_off_elsewhere() {
        for screen in SCREENS {
            let expected = if matches!(screen, Screen::Diff | Screen::Files) {
                Light::Available
            } else {
                Light::Off
            };
            assert_eq!(light(&ctx(screen), Key::Comment), expected, "{screen:?}");
        }
    }

    #[test]
    fn add_is_needed_only_when_composing_on_diff_or_files() {
        for screen in SCREENS {
            for composing in [false, true] {
                let c = Context {
                    composing,
                    ..ctx(screen)
                };
                let expected = if composing && matches!(screen, Screen::Diff | Screen::Files) {
                    Light::Needed
                } else {
                    Light::Off
                };
                assert_eq!(
                    light(&c, Key::Add),
                    expected,
                    "{screen:?} composing={composing}"
                );
            }
        }
    }

    #[test]
    fn submit_is_available_with_drafts_on_diff_or_files_off_elsewhere() {
        for screen in SCREENS {
            for draft_count in [0, 1] {
                let c = Context {
                    draft_count,
                    ..ctx(screen)
                };
                let expected = if draft_count > 0 && matches!(screen, Screen::Diff | Screen::Files)
                {
                    Light::Available
                } else {
                    Light::Off
                };
                assert_eq!(
                    light(&c, Key::Submit),
                    expected,
                    "{screen:?} drafts={draft_count}"
                );
            }
        }
    }

    #[test]
    fn accept_is_needed_on_outcome_when_comment_done() {
        for screen in SCREENS {
            for comment_done in [false, true] {
                let c = Context {
                    comment_done,
                    ..ctx(screen)
                };
                let expected = if screen == Screen::Outcome && comment_done {
                    Light::Needed
                } else {
                    Light::Off
                };
                assert_eq!(
                    light(&c, Key::Accept),
                    expected,
                    "{screen:?} done={comment_done}"
                );
            }
        }
    }

    #[test]
    fn reopen_is_available_on_outcome_when_comment_done() {
        for screen in SCREENS {
            for comment_done in [false, true] {
                let c = Context {
                    comment_done,
                    ..ctx(screen)
                };
                let expected = if screen == Screen::Outcome && comment_done {
                    Light::Available
                } else {
                    Light::Off
                };
                assert_eq!(
                    light(&c, Key::Reopen),
                    expected,
                    "{screen:?} done={comment_done}"
                );
            }
        }
    }

    #[test]
    fn stage_is_available_only_on_changes() {
        for screen in SCREENS {
            let expected = if screen == Screen::Changes {
                Light::Available
            } else {
                Light::Off
            };
            assert_eq!(light(&ctx(screen), Key::Stage), expected, "{screen:?}");
        }
    }

    #[test]
    fn commit_is_needed_on_changes_when_something_is_staged() {
        for screen in SCREENS {
            for staged_count in [0, 1] {
                let c = Context {
                    staged_count,
                    ..ctx(screen)
                };
                let expected = if screen == Screen::Changes && staged_count > 0 {
                    Light::Needed
                } else {
                    Light::Off
                };
                assert_eq!(
                    light(&c, Key::Commit),
                    expected,
                    "{screen:?} staged={staged_count}"
                );
            }
        }
    }

    #[test]
    fn undo_is_available_when_there_is_something_to_undo() {
        for screen in SCREENS {
            for can_undo in [false, true] {
                let c = Context {
                    can_undo,
                    ..ctx(screen)
                };
                let expected = if can_undo {
                    Light::Available
                } else {
                    Light::Off
                };
                assert_eq!(light(&c, Key::Undo), expected, "{screen:?} undo={can_undo}");
            }
        }
    }

    #[test]
    fn find_file_and_actions_are_available_on_every_screen() {
        for screen in SCREENS {
            assert_eq!(
                light(&ctx(screen), Key::FindFile),
                Light::Available,
                "{screen:?}"
            );
            assert_eq!(
                light(&ctx(screen), Key::Actions),
                Light::Available,
                "{screen:?}"
            );
        }
    }

    #[test]
    fn at_most_one_key_is_needed_in_any_context() {
        for c in all_contexts() {
            let needed = lights(&c).iter().filter(|l| **l == Light::Needed).count();
            assert!(needed <= 1, "{c:?} has {needed} needed keys");
        }
    }

    #[test]
    fn lights_matches_light_for_every_key() {
        for c in all_contexts() {
            let all = lights(&c);
            for (i, key) in Key::ALL.iter().enumerate() {
                assert_eq!(all[i], light(&c, *key), "{c:?} {key:?}");
            }
        }
    }

    #[test]
    fn all_has_thirteen_distinct_keys_in_group_order() {
        assert_eq!(Key::ALL.len(), 13);
        for (i, a) in Key::ALL.iter().enumerate() {
            for b in &Key::ALL[i + 1..] {
                assert_ne!(a, b);
            }
        }
        let rank = |g: Group| match g {
            Group::Move => 0,
            Group::Review => 1,
            Group::Git => 2,
            Group::Find => 3,
        };
        let ranks: Vec<u8> = Key::ALL.iter().map(|k| rank(k.group())).collect();
        assert!(
            ranks.windows(2).all(|w| w[0] <= w[1]),
            "groups interleave: {ranks:?}"
        );
    }

    #[test]
    fn hint_is_some_exactly_for_off_keys_with_a_hint_text() {
        let hinted = [
            Key::Move,
            Key::Open,
            Key::OpenFile,
            Key::Comment,
            Key::Add,
            Key::Submit,
            Key::Accept,
            Key::Reopen,
            Key::Stage,
            Key::Commit,
            Key::Undo,
        ];
        for c in all_contexts() {
            for key in Key::ALL {
                let expected = light(&c, key) == Light::Off && hinted.contains(&key);
                assert_eq!(hint(&c, key).is_some(), expected, "{c:?} {key:?}");
            }
        }
    }

    #[test]
    fn hint_texts_are_exact() {
        let c = ctx(Screen::Queue);
        assert_eq!(
            hint(&c, Key::OpenFile),
            Some("Open a file in the diff or the tree")
        );
        assert_eq!(
            hint(&c, Key::Comment),
            Some("Comments are written on a diff or a file")
        );
        assert_eq!(
            hint(&c, Key::Stage),
            Some("Stage changes on the Changes screen")
        );
        assert_eq!(
            hint(&ctx(Screen::Diff), Key::Open),
            Some("Select an item first")
        );
        assert_eq!(
            hint(&ctx(Screen::Diff), Key::Add),
            Some("Start a comment first")
        );
        assert_eq!(
            hint(&ctx(Screen::Diff), Key::Submit),
            Some("Add a comment to your review first")
        );
        assert_eq!(
            hint(&ctx(Screen::Diff), Key::Accept),
            Some("Select a comment the agent has done")
        );
        assert_eq!(
            hint(&ctx(Screen::Diff), Key::Reopen),
            Some("Select a comment the agent has done")
        );
        assert_eq!(
            hint(&ctx(Screen::Diff), Key::Commit),
            Some("Stage something to commit")
        );
        assert_eq!(hint(&ctx(Screen::Diff), Key::Undo), Some("Nothing to undo"));
    }

    #[test]
    fn hint_is_none_when_the_key_is_not_off() {
        let c = Context {
            can_undo: true,
            ..ctx(Screen::Diff)
        };
        assert_eq!(hint(&c, Key::Undo), None);
        assert_eq!(hint(&ctx(Screen::Search), Key::Open), None);
    }
}
