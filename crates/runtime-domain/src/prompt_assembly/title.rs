use super::DEFAULT_EXTRA_PROMPT_TITLE_PREFIX;

/// `derive_extra_prompt_title` 从 prompt body 提取列表展示标题。
#[must_use]
pub fn derive_extra_prompt_title(body: &str, fallback: &str) -> String {
    for line in body.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some(heading) = trimmed.strip_prefix('#') {
            let title = heading.trim_start_matches('#').trim();
            if !title.is_empty() {
                return truncate_extra_prompt_title(title);
            }
        }
        return truncate_extra_prompt_title(trimmed);
    }
    truncate_extra_prompt_title(fallback)
}

/// `next_default_extra_prompt_title` 为默认新建 prompt 生成递增标题。
#[must_use]
pub fn next_default_extra_prompt_title<'a>(titles: impl IntoIterator<Item = &'a str>) -> String {
    let next_index = titles
        .into_iter()
        .filter_map(default_extra_prompt_title_index)
        .max()
        .unwrap_or(0)
        .saturating_add(1);

    format!("{DEFAULT_EXTRA_PROMPT_TITLE_PREFIX} {next_index}")
}

/// `default_extra_prompt_body` 返回新建 extra prompt 的默认正文模板。
#[must_use]
pub fn default_extra_prompt_body(title: &str) -> String {
    format!("# {title}\n")
}

fn truncate_extra_prompt_title(title: &str) -> String {
    const TITLE_LIMIT: usize = 80;
    title.chars().take(TITLE_LIMIT).collect()
}

fn default_extra_prompt_title_index(title: &str) -> Option<usize> {
    if title == DEFAULT_EXTRA_PROMPT_TITLE_PREFIX {
        return Some(1);
    }

    let suffix = title
        .strip_prefix(DEFAULT_EXTRA_PROMPT_TITLE_PREFIX)?
        .strip_prefix(' ')?;
    suffix.parse::<usize>().ok().filter(|index| *index > 0)
}
