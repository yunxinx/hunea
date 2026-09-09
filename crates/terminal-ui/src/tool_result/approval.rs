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

/// pretty JSON 重排的字节门限。该检测落在 `measure_render_metrics` →
/// `approval_wrapped_styled_lines` 的渲染路径上，resize（width 变化）会对每个
/// Approval 结果全量重跑 parse + pretty + syntect 高亮；超大单行 payload 构成
/// 最坏情形，超过门限即回退 plain 渲染。
pub(super) const PRETTY_JSON_RESULT_MAX_BYTES: usize = 64 * 1024;

/// 展示侧 JSON 检测：单行（无换行）、以 `{`/`[` 开头、未超
/// `PRETTY_JSON_RESULT_MAX_BYTES` 门限且整体可解析的结果重排为 pretty 形态。
///
/// 只影响 TUI 呈现（紧凑单行 JSON 结果获得与代码块一致的观感），不改写 tool
/// result 进入模型上下文的文本；检测不满足或解析失败返回 `None`，调用方按
/// 原文本渲染。
pub(super) fn pretty_json_result_content(content: &str) -> Option<String> {
    let content = content.trim();
    if content.contains('\n')
        || !content.starts_with(['{', '['])
        || content.len() > PRETTY_JSON_RESULT_MAX_BYTES
    {
        return None;
    }
    let value = serde_json::from_str::<serde_json::Value>(content).ok()?;
    serde_json::to_string_pretty(&value).ok()
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
