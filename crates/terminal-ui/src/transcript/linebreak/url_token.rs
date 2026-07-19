//! URL-like token 判定（参考 codex-rs tui/src/wrapping.rs 的保守启发式）。
//!
//! 目标：真正的 URL 尽量整体不被切开；文件路径（如 `src/main.rs`）不算 URL。
//! planner 只消费 [`UrlTokenRange`]，具体判定规则全部收敛在本模块。

use std::ops::Range;

use url::Url;

use crate::display_width::display_width;

/// 一个 URL-like token 的原始范围与 body 范围。
///
/// `raw` 覆盖整个空白分隔 token（含包裹标点），`body` 只覆盖 URL 本体；
/// planner 用 `raw_width` 判断整个 token 能否原子地放进一行。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct UrlTokenRange {
    pub(super) raw: Range<usize>,
    pub(super) body: Range<usize>,
    pub(super) raw_width: usize,
}

/// 扫描空白分隔的 token，返回 raw token 与 URL body 的字节范围。
pub(super) fn url_token_ranges(text: &str) -> Vec<UrlTokenRange> {
    let mut ranges = Vec::new();
    let mut token_start: Option<usize> = None;

    for (offset, character) in text.char_indices() {
        if character.is_whitespace() {
            if let Some(start) = token_start.take() {
                push_if_url_like(text, start..offset, &mut ranges);
            }
        } else if token_start.is_none() {
            token_start = Some(offset);
        }
    }

    if let Some(start) = token_start {
        push_if_url_like(text, start..text.len(), &mut ranges);
    }

    ranges
}

/// 判断文本中是否存在 URL-like token（逐 token 早退，不收集范围）。
///
/// 供估算路径判定内容是否受 URL 原子保护影响；语义与 [`url_token_ranges`]
/// 的判定完全一致。
pub(super) fn has_url_like_token(text: &str) -> bool {
    text.split_whitespace()
        .any(|token| url_like_body_range(token).is_some())
}

fn push_if_url_like(text: &str, raw: Range<usize>, ranges: &mut Vec<UrlTokenRange>) {
    let raw_token = &text[raw.clone()];
    let Some(relative_body) = url_like_body_range(raw_token) else {
        return;
    };
    let body = raw.start + relative_body.start..raw.start + relative_body.end;
    ranges.push(UrlTokenRange {
        raw,
        body,
        raw_width: display_width(raw_token),
    });
}

/// 判断单个空白分隔 token 是否 URL-like。
#[cfg(test)]
fn is_url_like_token(raw_token: &str) -> bool {
    url_like_body_range(raw_token).is_some()
}

fn url_like_body_range(raw_token: &str) -> Option<Range<usize>> {
    let without_leading = raw_token.trim_start_matches(is_url_wrapper_punctuation);
    let start = raw_token.len().saturating_sub(without_leading.len());
    let body = without_leading.trim_end_matches(is_url_wrapper_punctuation);
    if body.is_empty() || !(is_absolute_url_like(body) || is_bare_url_like(body)) {
        return None;
    }

    Some(start..start + body.len())
}

fn is_url_wrapper_punctuation(character: char) -> bool {
    matches!(
        character,
        '(' | ')' | '[' | ']' | '{' | '}' | '<' | '>' | ',' | '.' | ';' | ':' | '!' | '\'' | '"'
    )
}

/// 形如 `scheme://...` 的绝对 URL。
///
/// 任意结构合法的 scheme 都有意放行（如 `vscode://file/a.rs`）：误判的代价只是
/// 该 token 整体不被切开，而收紧白名单会把真实的自定义协议链接拦腰截断。
/// 常见 web scheme 额外要求 host 存在，过滤 `http://` 这类残缺 token。
fn is_absolute_url_like(token: &str) -> bool {
    if !token.contains("://") {
        return false;
    }

    if let Ok(url) = Url::parse(token) {
        let scheme = url.scheme().to_ascii_lowercase();
        if matches!(
            scheme.as_str(),
            "http" | "https" | "ftp" | "ftps" | "ws" | "wss"
        ) {
            return url.host_str().is_some();
        }
        return true;
    }

    has_valid_scheme_prefix(token)
}

/// `url` crate 拒绝的自定义 scheme 兜底判定（如 `myapp://open/path`）。
fn has_valid_scheme_prefix(token: &str) -> bool {
    let Some((scheme, rest)) = token.split_once("://") else {
        return false;
    };
    if scheme.is_empty() || rest.is_empty() {
        return false;
    }

    let mut characters = scheme.chars();
    let Some(first) = characters.next() else {
        return false;
    };
    first.is_ascii_alphabetic()
        && characters.all(|character| {
            character.is_ascii_alphanumeric()
                || character == '+'
                || character == '-'
                || character == '.'
        })
}

/// 无 scheme 的裸域 URL：`host[:port]/path|?query|#frag`。
fn is_bare_url_like(token: &str) -> bool {
    let (host_port, has_trailer) = split_host_port_and_trailer(token);
    if host_port.is_empty() {
        return false;
    }

    // 裸 host 无路径时，只接受 `www.` 前缀，避免把 `hello.world` 误判为 URL。
    if !has_trailer && !host_port.to_ascii_lowercase().starts_with("www.") {
        return false;
    }

    let (host, port) = split_host_and_port(host_port);
    if host.is_empty() {
        return false;
    }
    if let Some(port) = port
        && !is_valid_port(port)
    {
        return false;
    }

    host.eq_ignore_ascii_case("localhost") || is_ipv4(host) || is_domain_name(host)
}

fn split_host_port_and_trailer(token: &str) -> (&str, bool) {
    if let Some(index) = token.find(['/', '?', '#']) {
        (&token[..index], true)
    } else {
        (token, false)
    }
}

/// 只在最后一个 `:` 处切分；端口合法性统一由 [`is_valid_port`] 判定。
fn split_host_and_port(host_port: &str) -> (&str, Option<&str>) {
    // 第一版不把带方括号的 IPv6 当作 URL-like。
    if host_port.starts_with('[') {
        return (host_port, None);
    }

    match host_port.rsplit_once(':') {
        Some((host, port)) => (host, Some(port)),
        None => (host_port, None),
    }
}

/// 端口必须是不超过 5 位的纯 ASCII 数字且落在 u16 范围内；
/// 单靠 `parse` 不够，它会接受前导 `+` 与前导零撑长的写法。
fn is_valid_port(port: &str) -> bool {
    if port.is_empty()
        || port.len() > 5
        || !port.chars().all(|character| character.is_ascii_digit())
    {
        return false;
    }

    port.parse::<u16>().is_ok()
}

fn is_ipv4(host: &str) -> bool {
    let parts: Vec<&str> = host.split('.').collect();
    if parts.len() != 4 {
        return false;
    }

    parts
        .iter()
        .all(|part| !part.is_empty() && part.parse::<u8>().is_ok())
}

fn is_domain_name(host: &str) -> bool {
    let host = host.to_ascii_lowercase();
    if !host.contains('.') {
        return false;
    }

    let mut labels = host.split('.');
    let Some(tld) = labels.next_back() else {
        return false;
    };
    if !is_tld(tld) {
        return false;
    }

    labels.all(is_domain_label)
}

fn is_tld(label: &str) -> bool {
    (2..=63).contains(&label.len())
        && label
            .chars()
            .all(|character| character.is_ascii_alphabetic())
}

fn is_domain_label(label: &str) -> bool {
    if label.is_empty() || label.len() > 63 {
        return false;
    }

    let Some(first) = label.chars().next() else {
        return false;
    };
    let Some(last) = label.chars().next_back() else {
        return false;
    };

    first.is_ascii_alphanumeric()
        && last.is_ascii_alphanumeric()
        && label
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '-')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_like_token_positive_cases() {
        for token in [
            "https://example.com/a/b",
            "ftp://host/path",
            "www.example.com/path?x=1",
            "localhost:3000/api",
            "127.0.0.1:8080/health",
            "(https://example.com/wrapped)",
        ] {
            assert!(is_url_like_token(token), "expected URL-like: {token:?}");
        }
    }

    #[test]
    fn url_like_token_negative_cases() {
        for token in [
            "src/main.rs",
            "./cmd/main.go",
            "foo/bar",
            "key:value",
            "hello.world",
            "localhost:+80/api",
        ] {
            assert!(
                !is_url_like_token(token),
                "did not expect URL-like: {token:?}"
            );
        }
    }
}
