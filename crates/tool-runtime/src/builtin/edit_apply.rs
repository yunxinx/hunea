use unicode_normalization::UnicodeNormalization;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct TextEdit {
    pub(super) old_string: String,
    pub(super) new_string: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum EditRequest {
    Single { edit: TextEdit, replace_all: bool },
    Multiple { edits: Vec<TextEdit> },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct EditApplication {
    pub(super) final_content: String,
    pub(super) replacements: usize,
}

pub(super) fn apply_edit(
    original: &str,
    request: &EditRequest,
    requested_path: &str,
) -> Result<EditApplication, String> {
    let (bom, content_without_bom) = strip_utf8_bom(original);
    let line_ending = detect_line_ending(content_without_bom);
    let normalized_content = normalize_line_endings(content_without_bom);

    let applied = match request {
        EditRequest::Single { edit, replace_all } => {
            let normalized_edit = NormalizedTextEdit::from_text_edit(edit);
            apply_single_edit_to_normalized_content(
                &normalized_content,
                &normalized_edit,
                *replace_all,
                requested_path,
            )?
        }
        EditRequest::Multiple { edits } => {
            let normalized_edits = edits
                .iter()
                .map(NormalizedTextEdit::from_text_edit)
                .collect::<Vec<_>>();
            apply_unique_edits_to_normalized_content(
                &normalized_content,
                &normalized_edits,
                requested_path,
            )?
        }
    };

    Ok(EditApplication {
        final_content: format!(
            "{bom}{}",
            restore_line_endings(&applied.content, line_ending)
        ),
        replacements: applied.replacements,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct NormalizedTextEdit {
    old_string: String,
    new_string: String,
}

impl NormalizedTextEdit {
    fn from_text_edit(edit: &TextEdit) -> Self {
        Self {
            old_string: normalize_line_endings(&edit.old_string),
            new_string: normalize_line_endings(&edit.new_string),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AppliedNormalizedContent {
    content: String,
    replacements: usize,
}

fn apply_single_edit_to_normalized_content(
    content: &str,
    edit: &NormalizedTextEdit,
    replace_all: bool,
    requested_path: &str,
) -> Result<AppliedNormalizedContent, String> {
    if edit.old_string.is_empty() {
        if !content.is_empty() {
            return Err("Cannot create new file - file already exists.".to_string());
        }
        return Ok(AppliedNormalizedContent {
            content: edit.new_string.clone(),
            replacements: 1,
        });
    }

    if replace_all {
        apply_replace_all_to_normalized_content(content, edit, requested_path)
    } else {
        apply_unique_edits_to_normalized_content(
            content,
            std::slice::from_ref(edit),
            requested_path,
        )
    }
}

fn apply_replace_all_to_normalized_content(
    content: &str,
    edit: &NormalizedTextEdit,
    requested_path: &str,
) -> Result<AppliedNormalizedContent, String> {
    let exact_occurrences = count_exact_occurrences(content, &edit.old_string);
    let fuzzy_occurrences = count_fuzzy_occurrences(content, &edit.old_string);
    if exact_occurrences == 0 && fuzzy_occurrences == 0 {
        return Err(not_found_error(&edit.old_string, None));
    }

    let use_fuzzy_base = fuzzy_occurrences > 0 && exact_occurrences != fuzzy_occurrences;
    let base_content = if use_fuzzy_base {
        normalize_for_fuzzy_match(content)
    } else {
        content.to_string()
    };
    let old_string = if use_fuzzy_base {
        normalize_for_fuzzy_match(&edit.old_string)
    } else {
        edit.old_string.clone()
    };

    if old_string.is_empty() {
        return Err(not_found_error(&edit.old_string, None));
    }

    let updated = base_content.replace(&old_string, &edit.new_string);
    if updated == base_content {
        return Err(format!("No changes made to {requested_path}."));
    }

    Ok(AppliedNormalizedContent {
        content: updated,
        replacements: if use_fuzzy_base {
            fuzzy_occurrences
        } else {
            exact_occurrences
        },
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MatchedTextEdit<'a> {
    edit_index: usize,
    match_index: usize,
    match_len: usize,
    new_string: &'a str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TextMatch {
    index: usize,
    len: usize,
    used_fuzzy_match: bool,
}

fn apply_unique_edits_to_normalized_content(
    content: &str,
    edits: &[NormalizedTextEdit],
    requested_path: &str,
) -> Result<AppliedNormalizedContent, String> {
    for (index, edit) in edits.iter().enumerate() {
        if edit.old_string.is_empty() {
            return Err(empty_old_string_error(index, edits.len()));
        }
    }

    let uses_fuzzy_base = edits
        .iter()
        .filter_map(|edit| find_text(content, &edit.old_string))
        .any(|match_result| match_result.used_fuzzy_match);
    let base_content = if uses_fuzzy_base {
        normalize_for_fuzzy_match(content)
    } else {
        content.to_string()
    };

    let mut matched_edits = Vec::with_capacity(edits.len());
    for (index, edit) in edits.iter().enumerate() {
        let Some(match_result) = find_text(&base_content, &edit.old_string) else {
            return Err(not_found_error(
                &edit.old_string,
                edit_index(index, edits.len()),
            ));
        };

        let occurrences = count_replacement_occurrences(&base_content, &edit.old_string);
        if occurrences > 1 {
            return Err(duplicate_error(
                &edit.old_string,
                edit_index(index, edits.len()),
                occurrences,
            ));
        }

        matched_edits.push(MatchedTextEdit {
            edit_index: index,
            match_index: match_result.index,
            match_len: match_result.len,
            new_string: &edit.new_string,
        });
    }

    matched_edits.sort_by_key(|edit| edit.match_index);
    for pair in matched_edits.windows(2) {
        let previous = &pair[0];
        let current = &pair[1];
        if previous.match_index + previous.match_len > current.match_index {
            return Err(format!(
                "edits[{}] and edits[{}] overlap in {requested_path}. Merge them into one edit or target disjoint regions.",
                previous.edit_index, current.edit_index
            ));
        }
    }

    let mut updated = base_content.clone();
    for edit in matched_edits.iter().rev() {
        updated.replace_range(
            edit.match_index..edit.match_index + edit.match_len,
            edit.new_string,
        );
    }

    if updated == base_content {
        return Err(format!("No changes made to {requested_path}."));
    }

    Ok(AppliedNormalizedContent {
        content: updated,
        replacements: matched_edits.len(),
    })
}

fn find_text(content: &str, old_string: &str) -> Option<TextMatch> {
    if let Some(index) = content.find(old_string) {
        return Some(TextMatch {
            index,
            len: old_string.len(),
            used_fuzzy_match: false,
        });
    }

    let fuzzy_content = normalize_for_fuzzy_match(content);
    let fuzzy_old_string = normalize_for_fuzzy_match(old_string);
    if fuzzy_old_string.is_empty() {
        return None;
    }

    fuzzy_content
        .find(&fuzzy_old_string)
        .map(|index| TextMatch {
            index,
            len: fuzzy_old_string.len(),
            used_fuzzy_match: true,
        })
}

fn count_exact_occurrences(content: &str, needle: &str) -> usize {
    if needle.is_empty() {
        return 0;
    }
    content.match_indices(needle).count()
}

fn count_fuzzy_occurrences(content: &str, needle: &str) -> usize {
    let fuzzy_needle = normalize_for_fuzzy_match(needle);
    if fuzzy_needle.is_empty() {
        return 0;
    }
    normalize_for_fuzzy_match(content)
        .match_indices(&fuzzy_needle)
        .count()
}

fn count_replacement_occurrences(content: &str, needle: &str) -> usize {
    let fuzzy_occurrences = count_fuzzy_occurrences(content, needle);
    if fuzzy_occurrences > 0 {
        fuzzy_occurrences
    } else {
        count_exact_occurrences(content, needle)
    }
}

fn edit_index(index: usize, edit_count: usize) -> Option<usize> {
    (edit_count > 1).then_some(index)
}

fn empty_old_string_error(index: usize, edit_count: usize) -> String {
    if edit_count == 1 {
        "old_string must not be empty.".to_string()
    } else {
        format!("edits[{index}].old_string must not be empty")
    }
}

fn not_found_error(old_string: &str, edit_index: Option<usize>) -> String {
    match edit_index {
        Some(index) => {
            format!(
                "String to replace not found in file for edits[{index}].old_string.\nString: {old_string}"
            )
        }
        None => format!("String to replace not found in file.\nString: {old_string}"),
    }
}

fn duplicate_error(old_string: &str, edit_index: Option<usize>, occurrences: usize) -> String {
    match edit_index {
        Some(index) => {
            format!(
                "Found {occurrences} matches of edits[{index}].old_string. Each old_string must be unique. Provide more context.\nString: {old_string}"
            )
        }
        None => {
            format!(
                "Found {occurrences} matches of the string to replace, but replace_all is false. To replace all occurrences, set replace_all to true. To replace only one occurrence, provide more context.\nString: {old_string}"
            )
        }
    }
}

fn normalize_for_fuzzy_match(text: &str) -> String {
    let normalized = text.nfkc().collect::<String>();
    normalized
        .split('\n')
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n")
        .chars()
        .map(normalize_fuzzy_char)
        .collect()
}

fn normalize_fuzzy_char(character: char) -> char {
    match character {
        '\u{2018}' | '\u{2019}' | '\u{201a}' | '\u{201b}' => '\'',
        '\u{201c}' | '\u{201d}' | '\u{201e}' | '\u{201f}' => '"',
        '\u{2010}' | '\u{2011}' | '\u{2012}' | '\u{2013}' | '\u{2014}' | '\u{2015}'
        | '\u{2212}' => '-',
        '\u{00a0}' | '\u{2002}'..='\u{200a}' | '\u{202f}' | '\u{205f}' | '\u{3000}' => ' ',
        other => other,
    }
}

fn strip_utf8_bom(text: &str) -> (&str, &str) {
    text.strip_prefix('\u{feff}')
        .map(|rest| ("\u{feff}", rest))
        .unwrap_or(("", text))
}

fn detect_line_ending(text: &str) -> LineEnding {
    match (text.find("\r\n"), text.find('\n')) {
        (Some(crlf), Some(lf)) if crlf < lf => LineEnding::CrLf,
        _ => LineEnding::Lf,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LineEnding {
    Lf,
    CrLf,
}

fn normalize_line_endings(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

fn restore_line_endings(text: &str, line_ending: LineEnding) -> String {
    match line_ending {
        LineEnding::Lf => text.to_string(),
        LineEnding::CrLf => text.replace('\n', "\r\n"),
    }
}
