use std::path::{Path, PathBuf};

use percent_encoding::percent_decode_str;
use url::Url;

pub(super) fn render_local_link_target(destination: &str, cwd: Option<&Path>) -> Option<String> {
    if !is_local_path_like_link(destination) {
        return None;
    }

    let (path_text, location_suffix) = parse_local_link_target(destination)?;
    let mut rendered = display_local_link_path(&path_text, cwd);
    if let Some(suffix) = location_suffix {
        rendered.push_str(&suffix);
    }
    Some(rendered)
}

fn is_local_path_like_link(destination: &str) -> bool {
    destination.starts_with("file://")
        || destination.starts_with('/')
        || destination.starts_with("~/")
        || destination.starts_with("./")
        || destination.starts_with("../")
        || destination.starts_with("\\\\")
        || matches!(
            destination.as_bytes(),
            [drive, b':', separator, ..]
                if drive.is_ascii_alphabetic() && matches!(separator, b'/' | b'\\')
        )
}

fn parse_local_link_target(destination: &str) -> Option<(String, Option<String>)> {
    if destination.starts_with("file://") {
        let url = Url::parse(destination).ok()?;
        let path = file_url_to_local_path_text(&url)?;
        let suffix = url
            .fragment()
            .and_then(normalize_hash_location_suffix_fragment);
        return Some((path, suffix));
    }

    let mut path_text = destination;
    let mut location_suffix = None;

    if let Some((candidate_path, fragment)) = destination.rsplit_once('#')
        && let Some(normalized) = normalize_hash_location_suffix_fragment(fragment)
    {
        path_text = candidate_path;
        location_suffix = Some(normalized);
    }

    if location_suffix.is_none()
        && let Some(suffix) = extract_colon_location_suffix(path_text)
    {
        let path_len = path_text.len().saturating_sub(suffix.len());
        path_text = &path_text[..path_len];
        location_suffix = Some(suffix.to_string());
    }

    let decoded_path_text = percent_decode_str(path_text)
        .decode_utf8()
        .unwrap_or(std::borrow::Cow::Borrowed(path_text));
    Some((expand_local_link_path(&decoded_path_text), location_suffix))
}

fn file_url_to_local_path_text(url: &Url) -> Option<String> {
    if let Ok(path) = url.to_file_path() {
        return Some(normalize_local_link_path_text(&path.to_string_lossy()));
    }

    let mut path_text = url.path().to_string();
    if let Some(host) = url.host_str()
        && !host.is_empty()
        && host != "localhost"
    {
        path_text = format!("//{host}{path_text}");
    } else if matches!(
        path_text.as_bytes(),
        [b'/', drive, b':', b'/', ..] if drive.is_ascii_alphabetic()
    ) {
        path_text.remove(0);
    }

    Some(normalize_local_link_path_text(&path_text))
}

fn normalize_hash_location_suffix_fragment(fragment: &str) -> Option<String> {
    let (start, end) = fragment
        .split_once("-L")
        .map_or((fragment, None), |(start, end)| (start, Some(end)));
    let start = parse_hash_location_part(start)?;
    let mut suffix = format!(":{}", start.line);
    if let Some(column) = start.column {
        suffix.push_str(&format!(":{column}"));
    }

    if let Some(end) = end {
        let end = parse_hash_location_part(&format!("L{end}"))?;
        suffix.push_str(&format!("-{}", end.line));
        if let Some(column) = end.column {
            suffix.push_str(&format!(":{column}"));
        }
    }

    Some(suffix)
}

#[derive(Debug, Clone, Copy)]
struct HashLocationPart {
    line: usize,
    column: Option<usize>,
}

fn parse_hash_location_part(part: &str) -> Option<HashLocationPart> {
    let rest = part.strip_prefix('L')?;
    let (line, column) = rest
        .split_once('C')
        .map_or((rest, None), |(line, column)| (line, Some(column)));
    if line.is_empty() || !line.chars().all(|character| character.is_ascii_digit()) {
        return None;
    }
    let column = match column {
        Some(column) => {
            if column.is_empty() || !column.chars().all(|character| character.is_ascii_digit()) {
                return None;
            }
            Some(column.parse().ok()?)
        }
        None => None,
    };

    Some(HashLocationPart {
        line: line.parse().ok()?,
        column,
    })
}

fn extract_colon_location_suffix(path_text: &str) -> Option<&str> {
    path_text
        .match_indices(':')
        .filter_map(|(index, _)| {
            let suffix = &path_text[index..];
            is_colon_location_suffix(suffix).then_some(suffix)
        })
        .next()
}

fn is_colon_location_suffix(suffix: &str) -> bool {
    let Some(rest) = suffix.strip_prefix(':') else {
        return false;
    };
    let (start, end) = rest
        .split_once(['-', '–'])
        .map_or((rest, None), |(start, end)| (start, Some(end)));

    is_line_column(start) && end.is_none_or(is_line_column)
}

fn is_line_column(text: &str) -> bool {
    let mut parts = text.split(':');
    let Some(line) = parts.next() else {
        return false;
    };
    if line.is_empty() || !line.chars().all(|character| character.is_ascii_digit()) {
        return false;
    }

    if let Some(column) = parts.next()
        && (column.is_empty() || !column.chars().all(|character| character.is_ascii_digit()))
    {
        return false;
    }

    parts.next().is_none()
}

fn expand_local_link_path(path_text: &str) -> String {
    if let Some(rest) = path_text.strip_prefix("~/")
        && let Some(home) = std::env::var_os("HOME")
    {
        return normalize_local_link_path_text(&PathBuf::from(home).join(rest).to_string_lossy());
    }

    normalize_local_link_path_text(path_text)
}

fn normalize_local_link_path_text(path_text: &str) -> String {
    if let Some(rest) = path_text.strip_prefix("\\\\") {
        format!("//{}", rest.replace('\\', "/").trim_start_matches('/'))
    } else {
        path_text.replace('\\', "/")
    }
}

fn display_local_link_path(path_text: &str, cwd: Option<&Path>) -> String {
    let path_text = normalize_local_link_path_text(path_text);
    if !is_absolute_local_link_path(&path_text) {
        return path_text;
    }

    if let Some(cwd) = cwd {
        let cwd_text = normalize_local_link_path_text(&cwd.to_string_lossy());
        if let Some(stripped) = strip_local_path_prefix(&path_text, &cwd_text) {
            return stripped.to_string();
        }
    }

    path_text
}

fn is_absolute_local_link_path(path_text: &str) -> bool {
    path_text.starts_with('/')
        || path_text.starts_with("//")
        || matches!(
            path_text.as_bytes(),
            [drive, b':', b'/', ..] if drive.is_ascii_alphabetic()
        )
}

fn strip_local_path_prefix<'a>(path_text: &'a str, cwd_text: &str) -> Option<&'a str> {
    let path_text = trim_trailing_local_path_separator(path_text);
    let cwd_text = trim_trailing_local_path_separator(cwd_text);

    if path_text == cwd_text {
        return None;
    }
    if cwd_text == "/" || cwd_text == "//" {
        return path_text.strip_prefix('/');
    }

    path_text
        .strip_prefix(cwd_text)
        .and_then(|rest| rest.strip_prefix('/'))
}

fn trim_trailing_local_path_separator(path_text: &str) -> &str {
    if path_text == "/" || path_text == "//" {
        return path_text;
    }
    if matches!(path_text.as_bytes(), [drive, b':', b'/'] if drive.is_ascii_alphabetic()) {
        return path_text;
    }
    path_text.trim_end_matches('/')
}
