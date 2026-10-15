use crate::{TextEdit, require_kind, safe_path};
use std::{
    fs::OpenOptions,
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
};

pub(crate) fn apply(root: &Path, relative: &Path, edits: &[TextEdit]) -> Result<(), String> {
    apply_checked(root, relative, edits)
        .map_err(|error| format!("edit_file {}: {error}", relative.display()))
}

fn apply_checked(root: &Path, relative: &Path, edits: &[TextEdit]) -> Result<(), String> {
    let path = safe_path(root, relative)?;
    require_kind(&path, false)?;
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
        .map_err(|error| format!("Cannot open file: {error}"))?;
    let mut original = String::new();
    file.read_to_string(&mut original)
        .map_err(|error| format!("Cannot read UTF-8 file: {error}"))?;
    let result = replace_text(&original, edits)?;

    // All decoding, match validation and result construction precede any mutation.
    // Keep the existing file (and permissions), as replace_file does. This is not
    // a crash-atomic write and cannot roll back an operating-system write failure.
    file.seek(SeekFrom::Start(0))
        .map_err(|error| format!("Cannot seek file: {error}"))?;
    file.set_len(0)
        .map_err(|error| format!("Cannot truncate file: {error}"))?;
    file.write_all(result.as_bytes())
        .map_err(|error| format!("Cannot write file: {error}"))
}

fn replace_text(original: &str, edits: &[TextEdit]) -> Result<String, String> {
    if edits.is_empty() {
        return Err("edits must not be empty".into());
    }
    let mut ranges = Vec::with_capacity(edits.len());
    for (index, edit) in edits.iter().enumerate() {
        let Some(first) = edit.old_text.chars().next() else {
            return Err(format!("edit {index}: oldText must not be empty"));
        };
        let Some(start) = original.find(edit.old_text.as_str()) else {
            return Err(format!("edit {index}: oldText match is missing"));
        };
        // Count overlapping occurrences too: aa occurs twice in aaa. Advancing
        // by one character keeps the suffix on a valid UTF-8 boundary.
        let next_start = start + first.len_utf8();
        if original[next_start..].contains(edit.old_text.as_str()) {
            return Err(format!("edit {index}: oldText match is ambiguous"));
        }
        ranges.push((start, start + edit.old_text.len(), index));
    }
    ranges.sort_unstable();
    for pair in ranges.windows(2) {
        if pair[0].1 > pair[1].0 {
            return Err(format!(
                "edits {} and {} have overlapping match ranges",
                pair[0].2, pair[1].2
            ));
        }
    }

    let mut result = String::with_capacity(original.len());
    let mut cursor = 0;
    for (start, end, index) in ranges {
        result.push_str(&original[cursor..start]);
        result.push_str(&edits[index].new_text);
        cursor = end;
    }
    result.push_str(&original[cursor..]);
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edits(pairs: &[(&str, &str)]) -> Vec<TextEdit> {
        pairs
            .iter()
            .map(|(old, new)| TextEdit {
                old_text: (*old).into(),
                new_text: (*new).into(),
            })
            .collect()
    }

    #[test]
    fn one_replacement_preserves_surrounding_content() {
        assert_eq!(
            replace_text("before old after", &edits(&[("old", "newer")])).unwrap(),
            "before newer after"
        );
    }

    #[test]
    fn multiple_edits_use_original_contents_in_either_order() {
        for pairs in [
            [("left", "right"), ("right", "done")],
            [("right", "done"), ("left", "right")],
        ] {
            assert_eq!(
                replace_text("left right", &edits(&pairs)).unwrap(),
                "right done"
            );
        }
    }

    #[test]
    fn earlier_replacements_cannot_create_later_matches() {
        let replacements = edits(&[("old", "fresh"), ("fresh", "done")]);
        let error = replace_text("old", &replacements).unwrap_err();
        assert!(error.contains("edit 1"), "{error}");
        assert!(error.contains("missing"), "{error}");
    }

    #[test]
    fn adjacent_ranges_are_allowed_even_in_reverse_order() {
        assert_eq!(
            replace_text("abcd", &edits(&[("cd", "!"), ("ab", "long")])).unwrap(),
            "long!"
        );
    }

    #[test]
    fn empty_inputs_fail_explicitly() {
        assert!(replace_text("abc", &[]).unwrap_err().contains("edits"));
        assert!(
            replace_text("abc", &edits(&[("", "x")]))
                .unwrap_err()
                .contains("oldText must not be empty")
        );
        assert!(
            replace_text("", &edits(&[("a", "x")]))
                .unwrap_err()
                .contains("missing")
        );
    }

    #[test]
    fn missing_and_ambiguous_matches_are_distinct() {
        for (original, old, reason) in [
            ("abc", "absent", "missing"),
            ("ab ab", "ab", "ambiguous"),
            ("aaa", "aa", "ambiguous"),
            ("ééé", "éé", "ambiguous"),
        ] {
            let error = replace_text(original, &edits(&[(old, "x")])).unwrap_err();
            assert!(error.contains("edit 0"), "{error}");
            assert!(error.contains(reason), "{error}");
        }
    }

    #[test]
    fn intersecting_nested_and_duplicate_ranges_fail() {
        for pairs in [
            vec![("abc", "x"), ("cde", "y")],
            vec![("abcde", "x"), ("b", "y")],
            vec![("abc", "x"), ("abc", "y")],
        ] {
            let error = replace_text("abcdef", &edits(&pairs)).unwrap_err();
            assert!(error.contains("overlapping"), "{error}");
            assert!(error.contains("0") && error.contains("1"), "{error}");
        }
    }

    #[test]
    fn matching_does_not_normalize_case_whitespace_or_line_endings() {
        for old in ["a\nb", "A", "a\t"] {
            assert!(replace_text("a\r\nb ", &edits(&[(old, "x")])).is_err());
        }
        assert_eq!(
            replace_text("a\r\nb ", &edits(&[("a\r\nb", "x")])).unwrap(),
            "x "
        );
    }

    #[test]
    fn unicode_deletion_and_untouched_bytes_are_exact() {
        let original = "\u{feff}α\r\nremove\n\0tail\r";
        let replacements = edits(&[("remove\n", ""), ("α", "β🙂")]);
        assert_eq!(
            replace_text(original, &replacements).unwrap(),
            "\u{feff}β🙂\r\n\0tail\r"
        );
        assert_eq!(replace_text("all", &edits(&[("all", "")])).unwrap(), "");
    }

    #[test]
    fn identical_replacement_is_valid() {
        assert_eq!(
            replace_text("same", &edits(&[("same", "same")])).unwrap(),
            "same"
        );
    }
}
