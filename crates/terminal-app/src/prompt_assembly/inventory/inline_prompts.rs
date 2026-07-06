use super::*;

pub(in crate::prompt_assembly) fn expand_custom_prompt_bindings(
    content: &str,
    bindings: &[TranscriptCustomPromptBinding],
    prompts_by_locator: &HashMap<(String, PromptSourceOrigin), PromptAssemblyExtraPromptCandidate>,
) -> Option<String> {
    let mut sorted_bindings = bindings.iter().collect::<Vec<_>>();
    sorted_bindings.sort_by_key(|binding| binding.start_char);

    let char_boundaries = content
        .char_indices()
        .map(|(index, _)| index)
        .chain(std::iter::once(content.len()))
        .collect::<Vec<_>>();

    let mut expanded = String::with_capacity(content.len());
    let mut cursor = 0;
    let mut replaced_any = false;

    for binding in sorted_bindings {
        let Some(prompt) = prompts_by_locator
            .iter()
            .find(|((reference_id, origin), _)| {
                reference_id.as_str() == binding.reference_id.as_str() && *origin == binding.origin
            })
            .map(|(_, prompt)| prompt)
        else {
            continue;
        };
        let trimmed_body = prompt.body.trim();
        if trimmed_body.is_empty() {
            continue;
        }

        let Some(&start_byte) = char_boundaries.get(binding.start_char) else {
            continue;
        };
        let Some(&end_byte) = char_boundaries.get(binding.end_char) else {
            continue;
        };
        if start_byte < cursor || end_byte < start_byte {
            continue;
        }

        expanded.push_str(&content[cursor..start_byte]);

        trim_trailing_inline_whitespace(&mut expanded);
        ensure_blank_line_before_inline_prompt(&mut expanded);
        expanded.push_str(trimmed_body);

        let skipped_after_bytes = count_leading_inline_whitespace(&content[end_byte..]);
        let trailing_text = &content[end_byte + skipped_after_bytes..];
        ensure_blank_line_after_inline_prompt(&mut expanded, trailing_text);

        cursor = end_byte + skipped_after_bytes;
        replaced_any = true;
    }

    if !replaced_any {
        return None;
    }

    expanded.push_str(&content[cursor..]);
    Some(expanded)
}

pub(in crate::prompt_assembly) fn trim_trailing_inline_whitespace(output: &mut String) {
    while matches!(output.chars().last(), Some(' ' | '\t')) {
        output.pop();
    }
}

pub(in crate::prompt_assembly) fn ensure_blank_line_before_inline_prompt(output: &mut String) {
    if output.is_empty() {
        return;
    }
    match trailing_newline_count(output) {
        0 => output.push_str("\n\n"),
        1 => output.push('\n'),
        _ => {}
    }
}

pub(in crate::prompt_assembly) fn ensure_blank_line_after_inline_prompt(
    output: &mut String,
    trailing_text: &str,
) {
    if trailing_text.is_empty() {
        return;
    }
    match leading_newline_count(trailing_text) {
        0 => output.push_str("\n\n"),
        1 => output.push('\n'),
        _ => {}
    }
}

pub(in crate::prompt_assembly) fn trailing_newline_count(value: &str) -> usize {
    value.chars().rev().take_while(|ch| *ch == '\n').count()
}

pub(in crate::prompt_assembly) fn leading_newline_count(value: &str) -> usize {
    value.chars().take_while(|ch| *ch == '\n').count()
}

pub(in crate::prompt_assembly) fn count_leading_inline_whitespace(value: &str) -> usize {
    value
        .char_indices()
        .take_while(|(_, ch)| matches!(ch, ' ' | '\t'))
        .map(|(index, ch)| index + ch.len_utf8())
        .last()
        .unwrap_or(0)
}
