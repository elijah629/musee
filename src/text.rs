pub fn clean_whitespace(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut in_whitespace = true;

    for ch in value.chars() {
        if ch.is_whitespace() {
            if !in_whitespace {
                out.push(' ');
                in_whitespace = true;
            }
        } else {
            out.push(ch);
            in_whitespace = false;
        }
    }

    if out.ends_with(' ') {
        out.pop();
    }

    out
}

pub fn normalize_text(value: &str, filesystem_safe: bool) -> String {
    let mut out = String::with_capacity(value.len());
    let mut prev_was_space = true;
    let mut prev_was_underscore = false;
    let chars: Vec<char> = value.chars().collect();

    for (index, ch) in chars.iter().copied().enumerate() {
        let ch = if ch == '\0' { ' ' } else { ch };
        if ch.is_whitespace() {
            if !prev_was_space && !out.is_empty() {
                out.push(' ');
                prev_was_space = true;
            }
            continue;
        }

        if ch == '_' {
            let next = chars.get(index + 1).copied();
            if prev_was_underscore {
                continue;
            }
            if prev_was_space && next.is_some_and(|c| c.is_alphanumeric()) {
                continue;
            }
            if next.is_none() && out.chars().next_back().is_some_and(|c| c.is_alphanumeric()) {
                continue;
            }
            if prev_was_space && next.is_some_and(|c| c.is_whitespace()) {
                continue;
            }
            out.push('_');
            prev_was_space = false;
            prev_was_underscore = true;
            continue;
        }

        let ch = if filesystem_safe {
            match ch {
                '/' | '\\' => '-',
                other => other,
            }
        } else {
            ch
        };

        out.push(ch);
        prev_was_space = false;
        prev_was_underscore = false;
    }

    while out.ends_with([' ', '.', '_']) {
        if out.ends_with('_')
            && !out[..out.len() - 1]
                .chars()
                .next_back()
                .is_some_and(|c| c.is_alphanumeric())
        {
            break;
        }
        out.pop();
    }

    let out = clean_whitespace(&out);
    if out.is_empty() {
        "Unknown".to_string()
    } else {
        out
    }
}

pub fn split_primary_artist(value: &str) -> String {
    let lower = value.to_ascii_lowercase();
    let bytes = lower.as_bytes();
    let mut cut = value.len();
    let mut i = 0;

    while i < bytes.len() {
        let matched = if bytes[i] == b';' || bytes[i] == b'&' || bytes[i] == b',' {
            Some(1)
        } else if starts_word(bytes, i, b"and") {
            Some(3)
        } else if starts_word(bytes, i, b"with") {
            Some(4)
        } else if starts_word(bytes, i, b"feat") {
            let extra = if bytes.get(i + 4) == Some(&b'.') {
                1
            } else {
                0
            };
            Some(4 + extra)
        } else if starts_word(bytes, i, b"featuring") {
            Some(9)
        } else {
            None
        };

        if let Some(len) = matched {
            cut = i;
            if cut > 0 && value.as_bytes()[cut - 1].is_ascii_whitespace() {
                cut -= 1;
            }
            let _ = len;
            break;
        }
        i += 1;
    }

    clean_whitespace(&value[..cut])
}

pub fn canonical_primary_artist(value: &str) -> String {
    let primary = normalize_text(&split_primary_artist(value), false);
    match primary.to_ascii_lowercase().as_str() {
        "ye" | "kanye west" => "Ye".to_string(),
        _ => primary,
    }
}

pub fn safe_name(value: &str) -> String {
    normalize_text(value, true)
}

pub fn track_prefix(tracknumber: Option<&str>) -> Option<String> {
    let tracknumber = tracknumber?;
    let digits: String = tracknumber
        .chars()
        .filter(|ch| ch.is_ascii_digit())
        .collect();
    if digits.is_empty() {
        return None;
    }
    let number: u32 = digits.parse().ok()?;
    Some(format!("{number:02}"))
}

fn starts_word(bytes: &[u8], index: usize, word: &[u8]) -> bool {
    if !bytes[index..].starts_with(word) {
        return false;
    }
    let before_ok = index == 0 || !bytes[index - 1].is_ascii_alphanumeric();
    let after = index + word.len();
    let after_ok = after >= bytes.len() || !bytes[after].is_ascii_alphanumeric();
    before_ok && after_ok
}

#[cfg(test)]
mod tests {
    use super::{normalize_text, split_primary_artist};

    #[test]
    fn trims_single_leading_underscore_artifact() {
        assert_eq!(normalize_text("_Name", false), "Name");
    }

    #[test]
    fn trims_single_trailing_underscore_artifact() {
        assert_eq!(normalize_text("Name_", false), "Name");
    }

    #[test]
    fn preserves_inner_underscores() {
        assert_eq!(normalize_text("A_B", false), "A_B");
    }

    #[test]
    fn collapses_whitespace_without_regex() {
        assert_eq!(normalize_text(" A   B \t C ", false), "A B C");
    }

    #[test]
    fn splits_primary_artist_on_feat_words() {
        assert_eq!(split_primary_artist("Artist feat. Guest"), "Artist");
    }
}
