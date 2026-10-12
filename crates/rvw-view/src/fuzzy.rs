//! Port of internal/tui/fuzzy.go: an fzf-style matcher, and the ranking of
//! candidates by it.
//!
//! Positions are indices into the text's `char`s, not bytes, so a caller can
//! highlight matches by walking `text.chars()`.

use std::cmp::Ordering;

/// Score of every matched character.
const SCORE_MATCH: i64 = 16;
/// The start of a path segment: after `/`, or the first character.
const BONUS_SEGMENT: i64 = 12;
/// The character after another separator (`_ - . space :`).
const BONUS_BOUNDARY: i64 = 8;
/// An upper-case letter after a lower-case one.
const BONUS_CAMEL: i64 = 7;
/// Right after the previous matched character.
const BONUS_CONSECUTIVE: i64 = 6;
/// Inside the last path segment, the file name.
const BONUS_FILE_NAME: i64 = 2;
const PENALTY_GAP_START: i64 = 3;
const PENALTY_GAP_EXTEND: i64 = 1;

/// Marks an impossible alignment cell. Any value at or below half of it is
/// unreachable; the arithmetic on it never overflows.
const NEG_INF: i64 = -(1 << 30);

/// A successful match: the best alignment's score and the char indices of
/// each pattern character in the text, in pattern order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Match {
    pub score: i64,
    pub positions: Vec<usize>,
}

/// One row a picker can show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    /// What is matched and shown.
    pub label: String,
    /// Open comments on the candidate; only whether it has any breaks ties.
    pub comments: usize,
}

/// A candidate that matched: its index in the input, and where.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ranked {
    pub index: usize,
    pub score: i64,
    pub positions: Vec<usize>,
}

/// Scores `text` against `pattern`, whose characters must appear in order,
/// case-insensitively. An empty pattern matches anything with score 0 and no
/// positions. Returns `None` when there is no match.
///
/// Matching is per `char` with simple (single-char) lower-casing, as Go's
/// `unicode.ToLower` does.
#[must_use]
pub fn fuzzy_match(pattern: &str, text: &str) -> Option<Match> {
    let p: Vec<char> = pattern.chars().map(simple_lower).collect();
    let t: Vec<char> = text.chars().collect();
    let (n, m) = (t.len(), p.len());
    if m == 0 {
        return Some(Match {
            score: 0,
            positions: Vec::new(),
        });
    }
    if m > n || !subsequence(&p, &t) {
        return None;
    }

    // Everything from the last '/' on is the file name. Without a '/' the
    // whole text is the name.
    let name_start = text
        .rfind('/')
        .map_or(0, |byte| text[..=byte].chars().count());
    let lower: Vec<char> = t.iter().copied().map(simple_lower).collect();
    let bonus: Vec<i64> = (0..n).map(|j| position_bonus(&t, j)).collect();
    let name: Vec<i64> = (0..n)
        .map(|j| if j >= name_start { BONUS_FILE_NAME } else { 0 })
        .collect();

    let table = align(&p, &lower, &bonus, &name);
    let (total, end) = best_end(&table, m)?;

    let mut positions = vec![0; m];
    let mut at = end;
    for i in (0..m).rev() {
        positions[i] = at;
        at = table.from[i][at];
    }
    Some(Match {
        score: total,
        positions,
    })
}

/// The dynamic-programming tables of one alignment. `score[i][j]` is the best
/// score with pattern char `i` matched at text char `j`; `from[i][j]` is where
/// char `i - 1` went on that best path; `chunk[i][j]` is the boundary bonus
/// the run of consecutive matches ending at `j` started with.
struct Table {
    score: Vec<Vec<i64>>,
    from: Vec<Vec<usize>>,
    chunk: Vec<Vec<i64>>,
}

/// Fills the alignment tables. `p` is the lower-cased pattern; `lower`,
/// `bonus` and `name` are per text char.
fn align(p: &[char], lower: &[char], bonus: &[i64], name: &[i64]) -> Table {
    let (m, n) = (p.len(), lower.len());
    let mut table = Table {
        score: vec![vec![NEG_INF; n]; m],
        from: vec![vec![0; n]; m],
        chunk: vec![vec![0; n]; m],
    };
    for (i, &pc) in p.iter().enumerate() {
        // The best score[i-1][k] minus the gap to reach j, for k < j - 1.
        let mut best = NEG_INF;
        let mut best_at = 0;
        for j in 0..n {
            if i > 0 && j >= 2 {
                best -= PENALTY_GAP_EXTEND;
                let s = table.score[i - 1][j - 2] - PENALTY_GAP_START;
                if s > best {
                    best = s;
                    best_at = j - 2;
                }
            }
            if lower[j] != pc {
                continue;
            }
            let fresh = SCORE_MATCH + bonus[j] + name[j];
            if i == 0 {
                table.score[i][j] = fresh;
                table.chunk[i][j] = bonus[j];
                continue;
            }
            if best > NEG_INF / 2 {
                table.score[i][j] = best + fresh;
                table.from[i][j] = best_at;
                table.chunk[i][j] = bonus[j];
            }
            if j > 0 && table.score[i - 1][j - 1] > NEG_INF / 2 {
                // Continuing a run keeps the boundary bonus it started on,
                // or takes a stronger one if it starts on a boundary.
                let mut first = table.chunk[i - 1][j - 1];
                if bonus[j] >= BONUS_BOUNDARY && bonus[j] > first {
                    first = bonus[j];
                }
                let run = bonus[j].max(first).max(BONUS_CONSECUTIVE);
                let s = table.score[i - 1][j - 1] + SCORE_MATCH + run + name[j];
                if s >= table.score[i][j] {
                    table.score[i][j] = s;
                    table.from[i][j] = j - 1;
                    table.chunk[i][j] = first;
                }
            }
        }
    }
    table
}

/// The last pattern char's best text position and score, if any alignment is
/// reachable. The earliest position wins a tie.
fn best_end(table: &Table, m: usize) -> Option<(i64, usize)> {
    let mut end = None;
    let mut total = NEG_INF;
    for (j, &s) in table.score[m - 1].iter().enumerate() {
        if s > total {
            total = s;
            end = Some(j);
        }
    }
    end.filter(|_| total > NEG_INF / 2).map(|j| (total, j))
}

/// Whether the lower-cased pattern `p` is a subsequence of the text `t`.
fn subsequence(p: &[char], t: &[char]) -> bool {
    let mut i = 0;
    for &r in t {
        if i < p.len() && simple_lower(r) == p[i] {
            i += 1;
        }
    }
    i == p.len()
}

/// The bonus for matching the char at `j`, from the char before it.
fn position_bonus(t: &[char], j: usize) -> i64 {
    if j == 0 {
        return BONUS_SEGMENT;
    }
    let (prev, cur) = (t[j - 1], t[j]);
    if prev == '/' {
        BONUS_SEGMENT
    } else if "_-. :".contains(prev) {
        BONUS_BOUNDARY
    } else if prev.is_lowercase() && cur.is_uppercase() {
        BONUS_CAMEL
    } else {
        0
    }
}

/// The single-char lower-case mapping of `c`: the first char of Rust's full
/// mapping, which is the simple mapping Go uses.
fn simple_lower(c: char) -> char {
    c.to_lowercase().next().unwrap_or(c)
}

/// Filters candidates by `query` and orders the matches best first: higher
/// score, then candidates with open comments, then shorter labels (in bytes),
/// then input order. An empty query keeps every candidate in input order.
#[must_use]
pub fn rank(query: &str, cands: &[Candidate]) -> Vec<Ranked> {
    let mut out: Vec<Ranked> = cands
        .iter()
        .enumerate()
        .filter_map(|(index, c)| {
            fuzzy_match(query, &c.label).map(|m| Ranked {
                index,
                score: m.score,
                positions: m.positions,
            })
        })
        .collect();
    if query.is_empty() {
        return out;
    }
    out.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then_with(|| compare_candidates(&cands[a.index], &cands[b.index]))
            .then_with(|| a.index.cmp(&b.index))
    });
    out
}

/// The tie-break order of [`rank`] after the score: a candidate with open
/// comments first, then the shorter label (in bytes). `Less` means `a` ranks
/// first. Equal candidates compare `Equal`, so a stable sort keeps input order.
#[must_use]
pub fn compare_candidates(a: &Candidate, b: &Candidate) -> Ordering {
    b.comments
        .min(1)
        .cmp(&a.comments.min(1))
        .then_with(|| a.label.len().cmp(&b.label.len()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels(query: &str, paths: &[&str], comments: &[(&str, usize)]) -> Vec<String> {
        let cands: Vec<Candidate> = paths
            .iter()
            .map(|p| Candidate {
                label: (*p).to_owned(),
                comments: comments.iter().find(|(l, _)| l == p).map_or(0, |(_, n)| *n),
            })
            .collect();
        rank(query, &cands)
            .into_iter()
            .map(|r| paths[r.index].to_owned())
            .collect()
    }

    #[test]
    fn rank_prefers_segment_starts_and_the_file_name() {
        let paths = [
            "test/api/api_parser_test",
            "docs/api/papers.md",
            "src/api/api.py",
            "src/handlers/v2/internal/api_parser.go",
            "src/api/parse.py",
        ];
        let got = labels("apipar", &paths, &[]);
        assert_eq!(got.first().map(String::as_str), Some("src/api/parse.py"));
        assert!(!got.iter().any(|p| p == "src/api/api.py"), "{got:?}");

        let got = labels("util", &["util/src/main.go", "src/util.go"], &[]);
        assert_eq!(got[0], "src/util.go");
    }

    #[test]
    fn rank_breaks_ties_by_comments_then_length() {
        let got = labels("x", &["b/x.go", "a/x.go", "c/x.gox"], &[("c/x.gox", 1)]);
        assert_eq!(got, ["c/x.gox", "b/x.go", "a/x.go"]);

        let got = labels("x", &["aa/x.go", "a/x.go"], &[]);
        assert_eq!(got[0], "a/x.go");

        assert_eq!(labels("", &["b", "a"], &[]), ["b", "a"]);
    }

    #[test]
    fn rank_counts_comments_as_a_flag_not_a_number() {
        // Two open comments do not outrank one: only "has any" breaks ties.
        let got = labels(
            "x",
            &["a/x.go", "b/x.go", "c/x.go"],
            &[("b/x.go", 1), ("c/x.go", 5)],
        );
        assert_eq!(got, ["b/x.go", "c/x.go", "a/x.go"]);
    }

    #[test]
    fn rank_reports_the_input_index_and_positions() {
        let cands = [
            Candidate {
                label: "zz".into(),
                comments: 0,
            },
            Candidate {
                label: "src/parse.py".into(),
                comments: 0,
            },
        ];
        let got = rank("par", &cands);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].index, 1);
        assert_eq!(got[0].positions, vec![4, 5, 6]);
    }

    #[test]
    fn empty_query_matches_everything_with_no_positions() {
        let cands = [
            Candidate {
                label: "b".into(),
                comments: 3,
            },
            Candidate {
                label: "a".into(),
                comments: 0,
            },
        ];
        let got = rank("", &cands);
        assert_eq!(got.len(), 2);
        assert!(got.iter().all(|r| r.positions.is_empty() && r.score == 0));
        assert_eq!(got[0].index, 0, "empty query keeps input order");
    }

    #[test]
    fn rank_of_no_candidates_or_no_matches_is_empty() {
        assert!(rank("q", &[]).is_empty());
        assert!(labels("zz", &["src/api.py"], &[]).is_empty());
    }

    #[test]
    fn fuzzy_match_reports_matched_chars() {
        let m = fuzzy_match("sPar", "src/api/parse.py").unwrap();
        assert_eq!(m.positions, vec![0, 8, 9, 10]);
        assert!(fuzzy_match("zz", "src/api.py").is_none());
    }

    #[test]
    fn empty_pattern_matches_with_score_zero() {
        let m = fuzzy_match("", "anything").unwrap();
        assert_eq!(m.score, 0);
        assert!(m.positions.is_empty());
        // An empty pattern matches even empty text.
        assert!(fuzzy_match("", "").is_some());
    }

    #[test]
    fn non_empty_pattern_never_matches_empty_text() {
        assert!(fuzzy_match("a", "").is_none());
    }

    #[test]
    fn pattern_longer_than_text_is_rejected() {
        assert!(fuzzy_match("abcd", "abc").is_none());
    }

    #[test]
    fn match_is_case_insensitive_both_ways() {
        assert!(fuzzy_match("API", "src/api.py").is_some());
        assert!(fuzzy_match("api", "SRC/API.PY").is_some());
    }

    #[test]
    fn positions_are_char_indices_not_bytes() {
        // 'é' is two bytes; the match of 'a' and 'b' sits at chars 1 and 2.
        let m = fuzzy_match("ab", "éab").unwrap();
        assert_eq!(m.positions, vec![1, 2]);

        let m = fuzzy_match("f", "日本/fé.txt").unwrap();
        assert_eq!(m.positions, vec![3]);
    }

    #[test]
    fn unicode_lower_casing_matches_case_insensitively() {
        let m = fuzzy_match("é", "CAFÉ").unwrap();
        assert_eq!(m.positions, vec![3]);
    }

    #[test]
    fn consecutive_run_scores_higher_than_a_gapped_one() {
        let run = fuzzy_match("abc", "xxabcxx").unwrap();
        let gapped = fuzzy_match("abc", "xxaxbxc").unwrap();
        assert!(run.score > gapped.score);
        assert_eq!(run.positions, vec![2, 3, 4]);
    }

    #[test]
    fn boundary_and_camel_bonuses_pick_the_word_start() {
        // "pa" aligns on the word starts: after "_" and at a camel-case "P".
        let m = fuzzy_match("pa", "some_parse_all").unwrap();
        assert_eq!(m.positions, vec![5, 6]);
        let m = fuzzy_match("pa", "xxxxxParseAll").unwrap();
        assert_eq!(m.positions, vec![5, 6]);
    }

    #[test]
    fn segment_start_beats_a_mid_word_match() {
        let start = fuzzy_match("p", "src/parse.py").unwrap();
        let mid = fuzzy_match("p", "srcparse.py").unwrap();
        assert!(start.score > mid.score);
    }

    #[test]
    fn file_name_beats_the_same_match_in_a_directory() {
        let in_name = fuzzy_match("u", "src/util.go").unwrap();
        let in_dir = fuzzy_match("u", "util/src.go").unwrap();
        assert!(in_name.score > in_dir.score);
    }

    #[test]
    fn compare_candidates_orders_by_comments_then_length() {
        let a = Candidate {
            label: "aa".into(),
            comments: 0,
        };
        let b = Candidate {
            label: "b".into(),
            comments: 0,
        };
        let c = Candidate {
            label: "ccc".into(),
            comments: 2,
        };
        assert_eq!(compare_candidates(&a, &b), Ordering::Greater);
        assert_eq!(compare_candidates(&c, &a), Ordering::Less);
    }
}
