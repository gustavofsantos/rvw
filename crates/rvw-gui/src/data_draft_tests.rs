//! Editing and deleting a draft comment, against a temporary store.

use rvw_core::operations::{PullInput, QueryInput};

use super::tests::workspace_with_store;
use super::*;

fn only_comment(service: &Service, identity: &Identity) -> Comment {
    let mut list = service
        .list(QueryInput {
            workspace: identity.workspace.clone(),
            status: "all".into(),
            ..QueryInput::default()
        })
        .expect("list")
        .comments;
    assert_eq!(list.len(), 1);
    list.remove(0)
}

#[test]
fn editing_changes_the_text_and_keeps_the_range() {
    let (_dir, service, identity) = workspace_with_store();
    add_comment(&service, &identity, "app.py", (1, 2), "first").expect("add");
    let edited = edit_comment(&service, &identity, "r1", "second\n").expect("edit");
    assert_eq!(edited.comment, "second");
    let stored = only_comment(&service, &identity);
    assert_eq!(stored.comment, "second");
    assert_eq!((stored.start_line, stored.end_line), (1, 2));
}

#[test]
fn a_blank_edit_is_refused_and_changes_nothing() {
    let (_dir, service, identity) = workspace_with_store();
    add_comment(&service, &identity, "app.py", (1, 1), "keep me").expect("add");
    let error = edit_comment(&service, &identity, "r1", " \n ").expect_err("blank");
    assert_eq!(error, "Write something, or delete the comment.");
    assert_eq!(only_comment(&service, &identity).comment, "keep me");
}

#[test]
fn removing_a_draft_deletes_it_and_returns_it() {
    let (_dir, service, identity) = workspace_with_store();
    add_comment(&service, &identity, "app.py", (2, 3), "drop this").expect("add");
    let removed = remove_comment(&service, &identity, "r1").expect("remove");
    assert_eq!(removed.id, "r1");
    assert_eq!(removed.comment, "drop this");
    let list = service
        .list(QueryInput {
            workspace: identity.workspace.clone(),
            status: "all".into(),
            ..QueryInput::default()
        })
        .expect("list");
    assert!(list.comments.is_empty());
}

#[test]
fn removing_or_editing_a_pulled_comment_returns_the_services_message() {
    let (_dir, service, identity) = workspace_with_store();
    add_comment(&service, &identity, "app.py", (1, 1), "taken").expect("add");
    service
        .pull(PullInput {
            workspace: identity.workspace.clone(),
            ..PullInput::default()
        })
        .expect("pull");
    let removal = remove_comment(&service, &identity, "r1").expect_err("pulled");
    assert!(removal.contains("only a pending comment can be removed"));
    let edit = edit_comment(&service, &identity, "r1", "new").expect_err("pulled");
    assert!(edit.contains("only a pending comment can be edited"));
    assert_eq!(only_comment(&service, &identity).comment, "taken");
}
