//! Commit history: a page of the log, one commit's details and diffs, and the
//! comparison of two revisions. Every read that can be slow takes a stop flag.

/// One commit as a log line shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogEntry {
    pub id: String,
    /// Parent ids, first parent first; empty for a root commit.
    pub parents: Vec<String>,
    pub author: String,
    pub email: String,
    /// Author time, seconds since the Unix epoch.
    pub time: i64,
    /// Ref names pointing at the commit as `%D` prints them, e.g.
    /// `["HEAD -> main", "origin/main", "tag: v1"]`.
    pub refs: Vec<String>,
    pub subject: String,
}

/// The `--format` that [`parse_log`] reads: fields split by 0x1f, records
/// ended by 0x1e.
pub(crate) const LOG_FORMAT: &str = "--format=%H%x1f%P%x1f%an%x1f%ae%x1f%at%x1f%D%x1f%s%x1e";

/// The entries of `git log` output written with [`LOG_FORMAT`]. A record that
/// does not have seven fields is skipped.
pub(crate) fn parse_log(out: &str) -> Vec<LogEntry> {
    out.split('\x1e')
        .filter_map(|record| {
            let record = record.trim_start_matches('\n');
            let fields: Vec<&str> = record.split('\x1f').collect();
            let [id, parents, author, email, time, refs, subject] = fields[..] else {
                return None;
            };
            Some(LogEntry {
                id: id.to_owned(),
                parents: parents.split_whitespace().map(str::to_owned).collect(),
                author: author.to_owned(),
                email: email.to_owned(),
                time: time.parse().unwrap_or(0),
                refs: refs
                    .split(", ")
                    .filter(|r| !r.is_empty())
                    .map(str::to_owned)
                    .collect(),
                subject: subject.to_owned(),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_records_with_and_without_parents_and_refs() {
        let out = "a1\x1fb2 c3\x1fAna\x1fana@x\x1f100\x1fHEAD -> main, tag: v1\x1fmerge it\x1e\n\
                   b2\x1f\x1fBo\x1fbo@x\x1f50\x1f\x1froot\x1e\n";
        let entries = parse_log(out);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].parents, ["b2", "c3"]);
        assert_eq!(entries[0].refs, ["HEAD -> main", "tag: v1"]);
        assert_eq!(entries[0].time, 100);
        assert!(entries[1].parents.is_empty());
        assert!(entries[1].refs.is_empty());
        assert_eq!(entries[1].subject, "root");
    }
}
