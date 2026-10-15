//! Writing a project file: adding and removing a `[[lane]]` as text. The edit
//! goes through `toml_edit`, so every byte outside it (comments, key order,
//! blank lines) stays as the user wrote it.

use toml_edit::{Array, ArrayOfTables, DocumentMut, Item, Table, value};

use crate::{Error, Lane};

/// `text` with `lane` appended as a `[[lane]]` at the end of the file.
pub(crate) fn add_lane(text: &str, lane: &Lane) -> Result<String, Error> {
    let doc = parse_doc(text)?;
    let taken = lanes(&doc)?.is_some_and(|existing| {
        existing
            .iter()
            .any(|table| str_field(table, "name") == Some(lane.name.as_str()))
    });
    if taken {
        return Err(refuse("lane", format!("{} is already a lane", lane.name)));
    }
    let repo_known = doc
        .get("repos")
        .and_then(Item::as_table_like)
        .is_some_and(|repos| repos.get(&lane.repo).is_some());
    if !repo_known {
        return Err(refuse(
            "lane.repo",
            format!("{} is not a key of [repos]", lane.repo),
        ));
    }
    if let Some(after) = &lane.after {
        let known = lanes(&doc)?.is_some_and(|existing| {
            existing
                .iter()
                .any(|table| str_field(table, "name") == Some(after.as_str()))
        });
        if !known {
            return Err(refuse(
                "lane.after",
                format!("{after} is not a lane of the project"),
            ));
        }
    }
    for name in &lane.terminals {
        let known = doc
            .get("terminals")
            .and_then(Item::as_table_like)
            .is_some_and(|terminals| terminals.get(name).is_some());
        if !known {
            return Err(refuse(
                "lane.terminals",
                format!("{name} is not a key of [terminals]"),
            ));
        }
    }

    let entry = render_entry(lane);
    let mut out = String::with_capacity(text.len() + entry.len() + 2);
    out.push_str(text);
    out.push_str(separator_before_new_table(text));
    out.push_str(&entry);
    Ok(out)
}

/// `text` without the `[[lane]]` named `name`. Refused while another lane
/// stacks on it (`after = name`): stacked lanes come off top-down.
pub(crate) fn remove_lane(text: &str, name: &str) -> Result<String, Error> {
    let mut doc = parse_doc(text)?;
    let Some(all) = lanes(&doc)? else {
        return Err(refuse("lane", format!("{name} is not a lane")));
    };
    let index = all
        .iter()
        .position(|table| str_field(table, "name") == Some(name))
        .ok_or_else(|| refuse("lane", format!("{name} is not a lane")))?;
    let stacked: Vec<&str> = all
        .iter()
        .filter(|table| str_field(table, "after") == Some(name))
        .filter_map(|table| str_field(table, "name"))
        .collect();
    if !stacked.is_empty() {
        return Err(refuse(
            "lane",
            format!(
                "{name} cannot be removed while {} stacks on it; remove {} first",
                stacked.join(", "),
                stacked.join(", ")
            ),
        ));
    }

    let Some(Item::ArrayOfTables(lanes)) = doc.get_mut("lane") else {
        return Err(refuse("lane", format!("{name} is not a lane")));
    };
    lanes.remove(index);
    if lanes.is_empty() {
        doc.remove("lane");
    }
    Ok(doc.to_string())
}

fn parse_doc(text: &str) -> Result<DocumentMut, Error> {
    text.parse::<DocumentMut>()
        .map_err(|error| refuse("", error.to_string()))
}

/// The `[[lane]]` tables of `doc`; `None` when there are none yet.
fn lanes(doc: &DocumentMut) -> Result<Option<&ArrayOfTables>, Error> {
    match doc.get("lane") {
        None => Ok(None),
        Some(item) => item
            .as_array_of_tables()
            .map(Some)
            .ok_or_else(|| refuse("lane", "must be an array of [[lane]] tables".to_owned())),
    }
}

fn str_field<'a>(table: &'a Table, key: &str) -> Option<&'a str> {
    table.get(key).and_then(Item::as_str)
}

fn render_entry(lane: &Lane) -> String {
    let mut table = Table::new();
    table.insert("name", value(lane.name.as_str()));
    table.insert("repo", value(lane.repo.as_str()));
    if let Some(after) = &lane.after {
        table.insert("after", value(after.as_str()));
    }
    if !lane.terminals.is_empty() {
        let mut terminals = Array::new();
        for terminal in &lane.terminals {
            terminals.push(terminal.as_str());
        }
        table.insert("terminals", value(terminals));
    }
    let mut entries = ArrayOfTables::new();
    entries.push(table);
    let mut doc = DocumentMut::new();
    doc.insert("lane", Item::ArrayOfTables(entries));
    doc.to_string()
}

/// The blank line between the file's last line and a new `[[lane]]`: none for
/// an empty file or one that already ends in a blank line.
fn separator_before_new_table(text: &str) -> &'static str {
    if text.trim().is_empty() || text.ends_with("\n\n") {
        ""
    } else if text.ends_with('\n') {
        "\n"
    } else {
        "\n\n"
    }
}

fn refuse(key: &str, message: String) -> Error {
    Error {
        file: None,
        key: key.to_owned(),
        message,
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use super::{add_lane, remove_lane};
    use crate::{Lane, parse};

    const BASE: &str = r#"name = "payments"

[repos.api]
path = "~/work/api"

[terminals]
claude = "claude"

# the schema lane
[[lane]]
name = "like-schema"
repo = "api"

# stacked on the schema lane
[[lane]]
name = "likes-endpoint"
repo = "api"
after = "like-schema"
terminals = ["claude"]

# notes live on proto
[[lane]]
name = "proto-notes"
repo = "api"
"#;

    const NO_LANES: &str = r#"name = "tiny"

[repos.api]
path = "~/work/api"
"#;

    fn lane(name: &str) -> Lane {
        Lane {
            name: name.to_owned(),
            repo: "api".to_owned(),
            after: None,
            terminals: Vec::new(),
        }
    }

    fn home() -> tempfile::TempDir {
        let home = tempfile::tempdir().unwrap();
        fs::create_dir_all(home.path().join("work/api")).unwrap();
        home
    }

    #[test]
    fn add_appends_a_lane_and_keeps_every_other_byte() {
        let mut new = lane("fresh");
        new.after = Some("proto-notes".to_owned());
        new.terminals = vec!["claude".to_owned()];
        let out = add_lane(BASE, &new).unwrap();
        assert_eq!(
            out,
            format!(
                "{BASE}\n[[lane]]\nname = \"fresh\"\nrepo = \"api\"\nafter = \"proto-notes\"\nterminals = [\"claude\"]\n"
            )
        );
    }

    #[test]
    fn add_to_a_file_without_lanes_puts_the_first_after_a_blank_line() {
        let out = add_lane(NO_LANES, &lane("first")).unwrap();
        assert_eq!(
            out,
            "name = \"tiny\"\n\n[repos.api]\npath = \"~/work/api\"\n\n[[lane]]\nname = \"first\"\nrepo = \"api\"\n"
        );
    }

    #[test]
    fn add_refuses_a_file_without_the_repo() {
        let error = add_lane("", &lane("first")).unwrap_err();
        assert_eq!(error.key, "lane.repo");
    }

    #[test]
    fn remove_last_lane_keeps_a_trailing_comment() {
        let text = format!("{BASE}# end of file\n");
        let out = remove_lane(&text, "proto-notes").unwrap();
        assert!(
            out.ends_with("terminals = [\"claude\"]\n# end of file\n"),
            "{out}"
        );
    }

    #[test]
    fn add_refuses_a_duplicate_name() {
        let error = add_lane(BASE, &lane("likes-endpoint")).unwrap_err();
        assert_eq!(error.key, "lane");
        assert_eq!(error.message, "likes-endpoint is already a lane");
    }

    #[test]
    fn add_refuses_a_repo_that_is_not_listed() {
        let mut new = lane("fresh");
        new.repo = "web".to_owned();
        let error = add_lane(BASE, &new).unwrap_err();
        assert_eq!(error.key, "lane.repo");
        assert_eq!(error.message, "web is not a key of [repos]");
    }

    #[test]
    fn remove_middle_lane_keeps_the_neighbours_comments() {
        let out = remove_lane(BASE, "likes-endpoint").unwrap();
        assert_eq!(
            out,
            r#"name = "payments"

[repos.api]
path = "~/work/api"

[terminals]
claude = "claude"

# the schema lane
[[lane]]
name = "like-schema"
repo = "api"

# notes live on proto
[[lane]]
name = "proto-notes"
repo = "api"
"#
        );
    }

    #[test]
    fn remove_the_last_lane_keeps_the_text_before_it() {
        let out = remove_lane(BASE, "proto-notes").unwrap();
        assert_eq!(
            out,
            r#"name = "payments"

[repos.api]
path = "~/work/api"

[terminals]
claude = "claude"

# the schema lane
[[lane]]
name = "like-schema"
repo = "api"

# stacked on the schema lane
[[lane]]
name = "likes-endpoint"
repo = "api"
after = "like-schema"
terminals = ["claude"]
"#
        );
    }

    #[test]
    fn remove_refuses_an_unknown_name() {
        let error = remove_lane(BASE, "nope").unwrap_err();
        assert_eq!(error.key, "lane");
        assert_eq!(error.message, "nope is not a lane");
    }

    #[test]
    fn remove_refuses_a_lane_another_lane_stacks_on() {
        let error = remove_lane(BASE, "like-schema").unwrap_err();
        assert_eq!(error.key, "lane");
        assert_eq!(
            error.message,
            "like-schema cannot be removed while likes-endpoint stacks on it; remove likes-endpoint first"
        );
    }

    #[test]
    fn removing_the_only_lane_drops_the_array() {
        let text = format!("{NO_LANES}\n[[lane]]\nname = \"only\"\nrepo = \"api\"\n");
        let out = remove_lane(&text, "only").unwrap();
        assert_eq!(out, NO_LANES);
    }

    #[test]
    fn add_then_remove_gives_back_the_original_text() {
        let added = add_lane(BASE, &lane("fresh")).unwrap();
        assert_eq!(remove_lane(&added, "fresh").unwrap(), BASE);
    }

    #[test]
    fn results_reparse_with_the_repo_dirs_present() {
        let home = home();
        let file = Path::new("/projects/payments.toml");
        let mut new = lane("fresh");
        new.after = Some("proto-notes".to_owned());
        new.terminals = vec!["claude".to_owned()];

        let added = add_lane(BASE, &new).unwrap();
        let project = parse(&added, Some(file), home.path()).unwrap();
        let names: Vec<&str> = project
            .lanes
            .iter()
            .map(|lane| lane.name.as_str())
            .collect();
        assert_eq!(
            names,
            ["like-schema", "likes-endpoint", "proto-notes", "fresh"]
        );

        let removed = remove_lane(&added, "likes-endpoint").unwrap();
        let project = parse(&removed, Some(file), home.path()).unwrap();
        let names: Vec<&str> = project
            .lanes
            .iter()
            .map(|lane| lane.name.as_str())
            .collect();
        assert_eq!(names, ["like-schema", "proto-notes", "fresh"]);
    }

    #[test]
    fn add_refuses_an_after_or_terminal_the_file_lacks() {
        let lane = |after: Option<&str>, terminals: &[&str]| Lane {
            name: "new".to_owned(),
            repo: "api".to_owned(),
            after: after.map(str::to_owned),
            terminals: terminals.iter().map(|t| (*t).to_owned()).collect(),
        };
        let error = add_lane(BASE, &lane(Some("ghost"), &[])).unwrap_err();
        assert_eq!(error.key, "lane.after");
        let error = add_lane(BASE, &lane(None, &["repl"])).unwrap_err();
        assert_eq!(error.key, "lane.terminals");
        assert!(add_lane(BASE, &lane(Some("like-schema"), &["claude"])).is_ok());
    }
}
