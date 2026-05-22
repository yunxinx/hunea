use ratatui::{
    style::{Modifier, Style},
    text::Span,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ParsedToolResultLine<'a> {
    pub(super) verb: &'a str,
    pub(super) body: &'a str,
    pub(super) should_highlight_as_shell: bool,
}

impl<'a> ParsedToolResultLine<'a> {
    pub(super) fn parse(content_line: &'a str) -> Option<Self> {
        let (verb, body) = split_verb(content_line)?;
        let body = body.trim_start();
        let (body, has_shell_prefix) = body
            .strip_prefix("Shell:")
            .map(|command| (command.trim_start(), true))
            .unwrap_or((body, false));
        let should_highlight_as_shell = has_shell_prefix || looks_like_shell_command(body);

        Some(Self {
            verb,
            body,
            should_highlight_as_shell,
        })
    }

    pub(super) fn non_shell_display_text(self) -> String {
        match self.verb {
            "Ran" => self.body.to_string(),
            "Reject" => {
                let rejected_body = strip_redundant_reject_title_verb(self.body);
                if rejected_body.is_empty() {
                    self.verb.to_string()
                } else {
                    format!("{} {}", self.verb, rejected_body)
                }
            }
            _ => {
                if self.body.is_empty() {
                    self.verb.to_string()
                } else {
                    format!("{} {}", self.verb, self.body)
                }
            }
        }
    }
}

fn split_verb(content_line: &str) -> Option<(&str, &str)> {
    for verb in ["Ran", "Reject"] {
        if content_line == verb {
            return Some((verb, ""));
        }
        if let Some(body) = content_line.strip_prefix(verb)
            && body.starts_with(char::is_whitespace)
        {
            return Some((verb, body));
        }
    }

    None
}

pub(super) fn looks_like_shell_command(body: &str) -> bool {
    let Some(first) = body.trim_start().chars().next() else {
        return false;
    };

    first.is_ascii_lowercase()
        || first.is_ascii_digit()
        || matches!(first, '.' | '/' | '~' | '$' | '\'' | '"' | '`')
}

fn strip_redundant_reject_title_verb(text: &str) -> &str {
    let text = text.trim_start();
    text.strip_prefix("Run ")
        .map(str::trim_start)
        .unwrap_or(text)
}

pub(super) fn style_core_result_line(line: String) -> Vec<Span<'static>> {
    let Some((core, rest)) = split_first_word(&line) else {
        return vec![Span::raw(line)];
    };

    if rest.is_empty() {
        return vec![Span::styled(
            core.to_string(),
            Style::new().add_modifier(Modifier::BOLD),
        )];
    }

    vec![
        Span::styled(core.to_string(), Style::new().add_modifier(Modifier::BOLD)),
        Span::raw(rest.to_string()),
    ]
}

fn split_first_word(line: &str) -> Option<(&str, &str)> {
    if line.is_empty() {
        return None;
    }

    let Some((index, _)) = line.char_indices().find(|(_, ch)| ch.is_whitespace()) else {
        return Some((line, ""));
    };

    Some((&line[..index], &line[index..]))
}
