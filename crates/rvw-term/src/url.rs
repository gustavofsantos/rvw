//! Finding a URL under a cell of a snapshot.

use crate::snapshot::{CellFlags, Snapshot};

/// A URL and where it sits. Rows and columns are inclusive and 0-based; a URL
/// that wraps spans several rows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UrlMatch {
    pub url: String,
    pub start_row: usize,
    pub start_col: usize,
    pub end_row: usize,
    pub end_col: usize,
}

const SCHEMES: [&str; 3] = ["https://", "http://", "file://"];

fn is_url_char(c: char) -> bool {
    !c.is_whitespace() && !c.is_control() && !"<>\"`".contains(c)
}

fn scheme_at(chars: &[(char, usize, usize)], at: usize) -> Option<usize> {
    SCHEMES.iter().find_map(|scheme| {
        let n = scheme.len();
        let window = chars.get(at..at + n)?;
        window
            .iter()
            .zip(scheme.chars())
            .all(|(&(c, _, _), s)| c.to_ascii_lowercase() == s)
            .then_some(n)
    })
}

impl Snapshot {
    /// The http, https or file URL under the cell, with trailing punctuation
    /// and an unbalanced closing bracket left out. Follows rows that wrap.
    #[must_use]
    pub fn url_at(&self, row: usize, col: usize) -> Option<UrlMatch> {
        if row >= self.rows || col >= self.cols {
            return None;
        }
        let wraps = |r: usize| {
            self.row(r)
                .last()
                .is_some_and(|c| c.flags.contains(CellFlags::WRAPLINE))
        };
        let mut first = row;
        while first > 0 && wraps(first - 1) {
            first -= 1;
        }
        let mut last = row;
        while last + 1 < self.rows && wraps(last) {
            last += 1;
        }
        let mut chars: Vec<(char, usize, usize)> = Vec::new();
        let mut target = None;
        for r in first..=last {
            for (c, cell) in self.row(r).iter().enumerate() {
                let here = r == row && c == col;
                if cell.flags.contains(CellFlags::WIDE_SPACER) {
                    if here && !chars.is_empty() {
                        target = Some(chars.len() - 1);
                    }
                    continue;
                }
                if here {
                    target = Some(chars.len());
                }
                chars.push((cell.ch, r, c));
            }
        }
        let target = target?;
        if !is_url_char(chars[target].0) {
            return None;
        }
        let mut left = target;
        while left > 0 && is_url_char(chars[left - 1].0) {
            left -= 1;
        }
        let mut right = target;
        while right + 1 < chars.len() && is_url_char(chars[right + 1].0) {
            right += 1;
        }
        let (begin, scheme_len) = (left..=target).rev().find_map(|i| {
            let boundary = i == left || !chars[i - 1].0.is_alphanumeric();
            if boundary {
                scheme_at(&chars[..=right], i).map(|n| (i, n))
            } else {
                None
            }
        })?;
        let body = begin + scheme_len;
        let mut end = right;
        while end >= body {
            let c = chars[end].0;
            let count = |ch: char| chars[begin..=end].iter().filter(|x| x.0 == ch).count();
            let unbalanced = |open: char, close: char| c == close && count(close) > count(open);
            if ".,;:!?'".contains(c)
                || unbalanced('(', ')')
                || unbalanced('[', ']')
                || unbalanced('{', '}')
            {
                end -= 1;
            } else {
                break;
            }
        }
        if end < body || target > end {
            return None;
        }
        Some(UrlMatch {
            url: chars[begin..=end].iter().map(|x| x.0).collect(),
            start_row: chars[begin].1,
            start_col: chars[begin].2,
            end_row: chars[end].1,
            end_col: chars[end].2,
        })
    }
}
