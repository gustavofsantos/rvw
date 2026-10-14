//! The "points at" reference a comment may carry, as the people who type it
//! write it: `FILE:N` or `FILE:N-M`. Pure text in, `Location` or a one-line
//! message out; the service checks the file itself.

use rvw_core::types::Location;

/// What every refusal of the format says the form is.
const FORMAT: &str = "Points at needs FILE:N or FILE:N-M, such as src/service.rs:90-96";

/// Parses `FILE:N` or `FILE:N-M`. The file is everything before the last
/// colon, so `a:b/c.rs:3` names `a:b/c.rs`. Spaces around the file and the
/// numbers are ignored. A reversed range is returned as typed: the service
/// swaps it.
///
/// # Errors
///
/// Fails with a one-line message when there is no colon, the file is empty,
/// a line number is missing, not all digits, zero, or too large.
pub fn parse_points_at(spec: &str) -> Result<Location, String> {
    let Some((file, lines)) = spec.trim().rsplit_once(':') else {
        return Err(FORMAT.to_owned());
    };
    let file = file.trim();
    if file.is_empty() {
        return Err("Points at needs a file before the colon".to_owned());
    }
    let (start, end) = lines.split_once('-').unwrap_or((lines, lines));
    Ok(Location {
        file: file.to_owned(),
        start_line: line_number(start)?,
        end_line: line_number(end)?,
    })
}

/// One line number: digits only, at least 1.
fn line_number(text: &str) -> Result<i64, String> {
    let text = text.trim();
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return Err(FORMAT.to_owned());
    }
    let number: i64 = text
        .parse()
        .map_err(|_| "Points at line numbers are too large".to_owned())?;
    if number == 0 {
        return Err("Points at lines start at 1".to_owned());
    }
    Ok(number)
}

/// The reference as the people read it: `file:N`, or `file:N-M` for a range.
#[must_use]
pub fn label(loc: &Location) -> String {
    if loc.start_line == loc.end_line {
        format!("{}:{}", loc.file, loc.start_line)
    } else {
        format!("{}:{}-{}", loc.file, loc.start_line, loc.end_line)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(file: &str, start_line: i64, end_line: i64) -> Location {
        Location {
            file: file.into(),
            start_line,
            end_line,
        }
    }

    #[test]
    fn a_single_line_is_a_range_of_one() {
        assert_eq!(parse_points_at("src/a.rs:7"), Ok(at("src/a.rs", 7, 7)));
    }

    #[test]
    fn a_range_keeps_both_ends() {
        assert_eq!(
            parse_points_at("src/service.rs:90-96"),
            Ok(at("src/service.rs", 90, 96))
        );
    }

    #[test]
    fn the_last_colon_splits_so_a_colon_in_the_file_name_survives() {
        assert_eq!(parse_points_at("a:b/c.rs:3"), Ok(at("a:b/c.rs", 3, 3)));
        assert_eq!(parse_points_at("a:b/c.rs:3-4"), Ok(at("a:b/c.rs", 3, 4)));
    }

    #[test]
    fn spaces_around_the_file_and_the_numbers_are_ignored() {
        assert_eq!(
            parse_points_at("  src/a.rs : 3 - 5  "),
            Ok(at("src/a.rs", 3, 5))
        );
    }

    #[test]
    fn a_reversed_range_is_returned_as_typed() {
        assert_eq!(parse_points_at("a.rs:9-4"), Ok(at("a.rs", 9, 4)));
    }

    #[test]
    fn a_spec_without_a_colon_is_refused_with_the_form() {
        assert_eq!(parse_points_at("src/a.rs"), Err(FORMAT.to_owned()));
        assert_eq!(parse_points_at(""), Err(FORMAT.to_owned()));
    }

    #[test]
    fn an_empty_file_is_refused() {
        assert_eq!(
            parse_points_at(":3"),
            Err("Points at needs a file before the colon".to_owned())
        );
        assert_eq!(
            parse_points_at("   :3"),
            Err("Points at needs a file before the colon".to_owned())
        );
    }

    #[test]
    fn an_empty_or_missing_line_is_refused() {
        for spec in ["a.rs:", "a.rs: ", "a.rs:3-", "a.rs:-3", "a.rs:3-4-5"] {
            assert_eq!(parse_points_at(spec), Err(FORMAT.to_owned()), "{spec}");
        }
    }

    #[test]
    fn a_non_digit_line_is_refused() {
        for spec in ["a.rs:x", "a.rs:3a", "a.rs:+3", "a.rs:1.5", "a.rs:3-x"] {
            assert_eq!(parse_points_at(spec), Err(FORMAT.to_owned()), "{spec}");
        }
    }

    #[test]
    fn line_zero_is_refused_at_either_end() {
        for spec in ["a.rs:0", "a.rs:0-4", "a.rs:4-0"] {
            assert_eq!(
                parse_points_at(spec),
                Err("Points at lines start at 1".to_owned()),
                "{spec}"
            );
        }
    }

    #[test]
    fn a_number_too_large_for_a_line_is_refused() {
        assert_eq!(
            parse_points_at("a.rs:99999999999999999999"),
            Err("Points at line numbers are too large".to_owned())
        );
    }

    #[test]
    fn every_refusal_is_one_line() {
        for spec in [
            "",
            "a.rs",
            ":3",
            "a.rs:0",
            "a.rs:x",
            "a.rs:99999999999999999999",
        ] {
            let message = parse_points_at(spec).expect_err(spec);
            assert!(!message.contains('\n'), "{spec}");
        }
    }

    #[test]
    fn label_is_file_and_line_or_range() {
        assert_eq!(label(&at("src/service.rs", 90, 90)), "src/service.rs:90");
        assert_eq!(label(&at("src/service.rs", 90, 96)), "src/service.rs:90-96");
    }

    #[test]
    fn a_parsed_spec_labels_as_it_reads() {
        let loc = parse_points_at("a:b/c.rs:3-8").expect("parses");
        assert_eq!(label(&loc), "a:b/c.rs:3-8");
    }
}
