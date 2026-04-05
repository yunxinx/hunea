use ratatui::{
    style::{Color, Modifier, Style},
    text::Line,
};

/// `lines_to_plain_text` 把带样式的行序列压平成纯文本。
pub(crate) fn lines_to_plain_text(lines: &[Line<'_>]) -> String {
    lines
        .iter()
        .map(line_to_plain_text)
        .collect::<Vec<_>>()
        .join("\n")
}

/// `lines_to_ansi_text` 把带样式的行序列编码成 ANSI 文本。
pub(crate) fn lines_to_ansi_text(lines: &[Line<'_>]) -> String {
    let mut rendered = String::new();

    for (index, line) in lines.iter().enumerate() {
        let mut active_style = Style::new();

        for span in &line.spans {
            if span.style != active_style {
                push_style_escape(&mut rendered, span.style);
                active_style = span.style;
            }

            rendered.push_str(span.content.as_ref());
        }

        if active_style != Style::new() {
            rendered.push_str("\u{1b}[0m");
        }

        if index + 1 < lines.len() {
            rendered.push('\n');
        }
    }

    rendered
}

/// `line_to_plain_text` 把单行带样式文本压平成纯文本。
pub(crate) fn line_to_plain_text(line: &Line<'_>) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect::<String>()
}

/// `line_plain_text_len` 返回单行纯文本的 UTF-8 字节长度。
pub(crate) fn line_plain_text_len(line: &Line<'_>) -> usize {
    line.spans.iter().map(|span| span.content.len()).sum()
}

fn push_style_escape(rendered: &mut String, style: Style) {
    let mut codes = Vec::new();

    match style.fg {
        Some(Color::Reset) | None => {}
        Some(color) => codes.push(foreground_code(color)),
    }

    match style.bg {
        Some(Color::Reset) | None => {}
        Some(color) => codes.push(background_code(color)),
    }

    if style.add_modifier.contains(Modifier::BOLD) {
        codes.push(String::from("1"));
    }
    if style.add_modifier.contains(Modifier::ITALIC) {
        codes.push(String::from("3"));
    }
    if style.add_modifier.contains(Modifier::UNDERLINED) {
        codes.push(String::from("4"));
    }
    if style.add_modifier.contains(Modifier::CROSSED_OUT) {
        codes.push(String::from("9"));
    }

    if codes.is_empty() {
        rendered.push_str("\u{1b}[0m");
        return;
    }

    rendered.push_str("\u{1b}[");
    rendered.push_str(&codes.join(";"));
    rendered.push('m');
}

fn foreground_code(color: Color) -> String {
    match color {
        Color::Black => String::from("30"),
        Color::Red => String::from("31"),
        Color::Green => String::from("32"),
        Color::Yellow => String::from("33"),
        Color::Blue => String::from("34"),
        Color::Magenta => String::from("35"),
        Color::Cyan => String::from("36"),
        Color::Gray => String::from("37"),
        Color::DarkGray => String::from("90"),
        Color::LightRed => String::from("91"),
        Color::LightGreen => String::from("92"),
        Color::LightYellow => String::from("93"),
        Color::LightBlue => String::from("94"),
        Color::LightMagenta => String::from("95"),
        Color::LightCyan => String::from("96"),
        Color::White => String::from("97"),
        Color::Indexed(index) => format!("38;5;{index}"),
        Color::Rgb(red, green, blue) => format!("38;2;{red};{green};{blue}"),
        Color::Reset => String::from("39"),
    }
}

fn background_code(color: Color) -> String {
    match color {
        Color::Black => String::from("40"),
        Color::Red => String::from("41"),
        Color::Green => String::from("42"),
        Color::Yellow => String::from("43"),
        Color::Blue => String::from("44"),
        Color::Magenta => String::from("45"),
        Color::Cyan => String::from("46"),
        Color::Gray => String::from("47"),
        Color::DarkGray => String::from("100"),
        Color::LightRed => String::from("101"),
        Color::LightGreen => String::from("102"),
        Color::LightYellow => String::from("103"),
        Color::LightBlue => String::from("104"),
        Color::LightMagenta => String::from("105"),
        Color::LightCyan => String::from("106"),
        Color::White => String::from("107"),
        Color::Indexed(index) => format!("48;5;{index}"),
        Color::Rgb(red, green, blue) => format!("48;2;{red};{green};{blue}"),
        Color::Reset => String::from("49"),
    }
}
