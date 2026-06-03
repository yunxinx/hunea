use std::io::{self, Write};

use crossterm::{
    cursor::MoveTo,
    queue,
    style::{
        Attribute as CrosstermAttribute, Colors as CrosstermColors, Print, SetAttribute,
        SetBackgroundColor, SetColors, SetForegroundColor, SetUnderlineColor,
    },
    terminal::{Clear, ClearType as CrosstermClearType},
};
use ratatui::{
    backend::{Backend, ClearType, IntoCrossterm},
    buffer::{Buffer, Cell},
    layout::{Position, Rect, Size},
    style::{Color, Modifier},
};

use crate::terminal_grid::{TerminalDrawCommand, diff_terminal_buffers};

/// `TerminalSurface` 负责生产终端的双 buffer 差量刷新。
#[derive(Debug)]
pub(crate) struct TerminalSurface<B>
where
    B: Backend<Error = io::Error> + Write,
{
    backend: B,
    buffers: [Buffer; 2],
    current: usize,
    viewport_area: Rect,
    last_known_screen_size: Size,
    hidden_cursor: bool,
}

impl<B> TerminalSurface<B>
where
    B: Backend<Error = io::Error> + Write,
{
    pub(crate) fn new(backend: B) -> io::Result<Self> {
        let screen_size = backend.size()?;
        let viewport_area = Rect::new(0, 0, screen_size.width, screen_size.height);
        Ok(Self {
            backend,
            buffers: [Buffer::empty(viewport_area), Buffer::empty(viewport_area)],
            current: 0,
            viewport_area,
            last_known_screen_size: screen_size,
            hidden_cursor: false,
        })
    }

    pub(crate) const fn backend_mut(&mut self) -> &mut B {
        &mut self.backend
    }

    pub(crate) fn size(&self) -> io::Result<Size> {
        self.backend.size()
    }

    pub(crate) fn draw<F>(&mut self, render: F) -> io::Result<()>
    where
        F: FnOnce(Rect, &mut Buffer) -> Option<Position>,
    {
        self.autoresize()?;

        let viewport_area = self.viewport_area;
        let cursor_position = {
            let current = &mut self.buffers[self.current];
            current.resize(viewport_area);
            render(viewport_area, current)
        };

        self.flush_screen()?;

        match cursor_position {
            Some(position) => {
                self.show_cursor()?;
                self.set_cursor_position(position)?;
            }
            None => self.hide_cursor()?,
        }

        self.swap_buffers();
        Backend::flush(&mut self.backend)
    }

    pub(crate) fn hide_cursor(&mut self) -> io::Result<()> {
        if self.hidden_cursor {
            return Ok(());
        }
        self.backend.hide_cursor()?;
        self.hidden_cursor = true;
        Ok(())
    }

    pub(crate) fn show_cursor(&mut self) -> io::Result<()> {
        if !self.hidden_cursor {
            return Ok(());
        }
        self.backend.show_cursor()?;
        self.hidden_cursor = false;
        Ok(())
    }

    pub(crate) fn clear(&mut self) -> io::Result<()> {
        self.backend.clear_region(ClearType::All)?;
        self.buffers[0].reset();
        self.buffers[1].reset();
        Ok(())
    }

    pub(crate) fn last_frame_buffer(&self) -> &Buffer {
        &self.buffers[1 - self.current]
    }

    fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> io::Result<()> {
        self.backend.set_cursor_position(position)
    }

    fn autoresize(&mut self) -> io::Result<()> {
        let screen_size = self.backend.size()?;
        if screen_size == self.last_known_screen_size {
            return Ok(());
        }

        self.last_known_screen_size = screen_size;
        self.viewport_area = Rect::new(0, 0, screen_size.width, screen_size.height);
        self.buffers[0].resize(self.viewport_area);
        self.buffers[1].resize(self.viewport_area);
        self.backend.clear_region(ClearType::All)?;
        self.buffers[1 - self.current].reset();
        Ok(())
    }

    fn flush_screen(&mut self) -> io::Result<()> {
        let commands =
            diff_terminal_buffers(&self.buffers[1 - self.current], &self.buffers[self.current]);
        draw_terminal_commands(&mut self.backend, commands.into_iter())
    }

    fn swap_buffers(&mut self) {
        self.buffers[1 - self.current].reset();
        self.current = 1 - self.current;
    }
}

fn draw_terminal_commands<'a, W, I>(writer: &mut W, commands: I) -> io::Result<()>
where
    W: Write,
    I: Iterator<Item = TerminalDrawCommand<'a>>,
{
    let mut style = TerminalStyleState::default();
    let mut last_pos: Option<Position> = None;

    for command in commands {
        let (x, y) = match &command {
            TerminalDrawCommand::Put { x, y, .. }
            | TerminalDrawCommand::ClearToEnd { x, y, .. } => (*x, *y),
        };
        if !matches!(last_pos, Some(position) if x == position.x + 1 && y == position.y) {
            queue!(writer, MoveTo(x, y))?;
        }
        last_pos = Some(Position { x, y });

        match command {
            TerminalDrawCommand::Put {
                x,
                y,
                cell,
                prefill_width,
            } => {
                style.queue_cell(writer, cell)?;
                if prefill_width > 1 {
                    queue!(writer, Print(" ".repeat(prefill_width)), MoveTo(x, y))?;
                }
                queue!(writer, Print(cell.symbol()))?;
            }
            TerminalDrawCommand::ClearToEnd { bg: clear_bg, .. } => {
                style.reset(writer)?;
                style.queue_background(writer, clear_bg)?;
                queue!(writer, Clear(CrosstermClearType::UntilNewLine))?;
            }
        }
    }

    style.reset(writer)?;
    Ok(())
}

#[derive(Debug, Clone, Copy)]
struct TerminalStyleState {
    fg: Color,
    bg: Color,
    underline_color: Color,
    modifier: Modifier,
}

impl Default for TerminalStyleState {
    fn default() -> Self {
        Self {
            fg: Color::Reset,
            bg: Color::Reset,
            underline_color: Color::Reset,
            modifier: Modifier::empty(),
        }
    }
}

impl TerminalStyleState {
    fn queue_cell<W>(&mut self, writer: &mut W, cell: &Cell) -> io::Result<()>
    where
        W: Write,
    {
        if cell.modifier != self.modifier {
            ModifierDiff {
                from: self.modifier,
                to: cell.modifier,
            }
            .queue(writer)?;
            self.modifier = cell.modifier;
        }
        if cell.fg != self.fg || cell.bg != self.bg {
            queue!(
                writer,
                SetColors(CrosstermColors::new(
                    cell.fg.into_crossterm(),
                    cell.bg.into_crossterm(),
                ))
            )?;
            self.fg = cell.fg;
            self.bg = cell.bg;
        }
        if cell.underline_color != self.underline_color {
            queue!(
                writer,
                SetUnderlineColor(cell.underline_color.into_crossterm())
            )?;
            self.underline_color = cell.underline_color;
        }
        Ok(())
    }

    fn queue_background<W>(&mut self, writer: &mut W, bg: Color) -> io::Result<()>
    where
        W: Write,
    {
        if self.bg != bg {
            queue!(writer, SetBackgroundColor(bg.into_crossterm()))?;
            self.bg = bg;
        }
        Ok(())
    }

    fn reset<W>(&mut self, writer: &mut W) -> io::Result<()>
    where
        W: Write,
    {
        queue!(
            writer,
            SetForegroundColor(crossterm::style::Color::Reset),
            SetBackgroundColor(crossterm::style::Color::Reset),
            SetUnderlineColor(crossterm::style::Color::Reset),
            SetAttribute(CrosstermAttribute::Reset),
        )?;
        self.fg = Color::Reset;
        self.bg = Color::Reset;
        self.underline_color = Color::Reset;
        self.modifier = Modifier::empty();
        Ok(())
    }
}

struct ModifierDiff {
    from: Modifier,
    to: Modifier,
}

impl ModifierDiff {
    fn queue<W>(self, writer: &mut W) -> io::Result<()>
    where
        W: Write,
    {
        let removed = self.from - self.to;
        if removed.contains(Modifier::REVERSED) {
            queue!(writer, SetAttribute(CrosstermAttribute::NoReverse))?;
        }
        if removed.contains(Modifier::BOLD) || removed.contains(Modifier::DIM) {
            queue!(writer, SetAttribute(CrosstermAttribute::NormalIntensity))?;
            if self.to.contains(Modifier::DIM) {
                queue!(writer, SetAttribute(CrosstermAttribute::Dim))?;
            }
            if self.to.contains(Modifier::BOLD) {
                queue!(writer, SetAttribute(CrosstermAttribute::Bold))?;
            }
        }
        if removed.contains(Modifier::ITALIC) {
            queue!(writer, SetAttribute(CrosstermAttribute::NoItalic))?;
        }
        if removed.contains(Modifier::UNDERLINED) {
            queue!(writer, SetAttribute(CrosstermAttribute::NoUnderline))?;
        }
        if removed.contains(Modifier::CROSSED_OUT) {
            queue!(writer, SetAttribute(CrosstermAttribute::NotCrossedOut))?;
        }
        if removed.contains(Modifier::SLOW_BLINK) || removed.contains(Modifier::RAPID_BLINK) {
            queue!(writer, SetAttribute(CrosstermAttribute::NoBlink))?;
        }
        if removed.contains(Modifier::HIDDEN) {
            queue!(writer, SetAttribute(CrosstermAttribute::NoHidden))?;
        }

        let added = self.to - self.from;
        if added.contains(Modifier::REVERSED) {
            queue!(writer, SetAttribute(CrosstermAttribute::Reverse))?;
        }
        if added.contains(Modifier::BOLD) {
            queue!(writer, SetAttribute(CrosstermAttribute::Bold))?;
        }
        if added.contains(Modifier::ITALIC) {
            queue!(writer, SetAttribute(CrosstermAttribute::Italic))?;
        }
        if added.contains(Modifier::UNDERLINED) {
            queue!(writer, SetAttribute(CrosstermAttribute::Underlined))?;
        }
        if added.contains(Modifier::DIM) {
            queue!(writer, SetAttribute(CrosstermAttribute::Dim))?;
        }
        if added.contains(Modifier::CROSSED_OUT) {
            queue!(writer, SetAttribute(CrosstermAttribute::CrossedOut))?;
        }
        if added.contains(Modifier::SLOW_BLINK) {
            queue!(writer, SetAttribute(CrosstermAttribute::SlowBlink))?;
        }
        if added.contains(Modifier::RAPID_BLINK) {
            queue!(writer, SetAttribute(CrosstermAttribute::RapidBlink))?;
        }
        if added.contains(Modifier::HIDDEN) {
            queue!(writer, SetAttribute(CrosstermAttribute::Hidden))?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, rc::Rc};

    use crossterm::style::SetUnderlineColor;
    use ratatui::{backend::WindowSize, style::Style};

    use super::*;
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum CursorEvent {
        Hide,
        Show,
    }

    #[derive(Debug, Clone)]
    struct CapturedOutput(Rc<RefCell<Vec<u8>>>);

    impl CapturedOutput {
        fn text(&self) -> String {
            String::from_utf8_lossy(&self.0.borrow()).into_owned()
        }
    }

    struct CaptureBackend {
        output: CapturedOutput,
        cursor_events: Rc<RefCell<Vec<CursorEvent>>>,
        size: Size,
        cursor: Position,
    }

    impl CaptureBackend {
        fn new(width: u16, height: u16) -> (Self, CapturedOutput, Rc<RefCell<Vec<CursorEvent>>>) {
            let output = CapturedOutput(Rc::new(RefCell::new(Vec::new())));
            let cursor_events = Rc::new(RefCell::new(Vec::new()));
            (
                Self {
                    output: output.clone(),
                    cursor_events: Rc::clone(&cursor_events),
                    size: Size::new(width, height),
                    cursor: Position::ORIGIN,
                },
                output,
                cursor_events,
            )
        }
    }

    impl Write for CaptureBackend {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.output.0.borrow_mut().extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl Backend for CaptureBackend {
        type Error = io::Error;

        fn draw<'a, I>(&mut self, _content: I) -> Result<(), Self::Error>
        where
            I: Iterator<Item = (u16, u16, &'a Cell)>,
        {
            Ok(())
        }

        fn hide_cursor(&mut self) -> Result<(), Self::Error> {
            self.cursor_events.borrow_mut().push(CursorEvent::Hide);
            Ok(())
        }

        fn show_cursor(&mut self) -> Result<(), Self::Error> {
            self.cursor_events.borrow_mut().push(CursorEvent::Show);
            Ok(())
        }

        fn get_cursor_position(&mut self) -> Result<Position, Self::Error> {
            Ok(self.cursor)
        }

        fn set_cursor_position<P: Into<Position>>(
            &mut self,
            position: P,
        ) -> Result<(), Self::Error> {
            self.cursor = position.into();
            Ok(())
        }

        fn clear(&mut self) -> Result<(), Self::Error> {
            Ok(())
        }

        fn clear_region(&mut self, _clear_type: ClearType) -> Result<(), Self::Error> {
            Ok(())
        }

        fn size(&self) -> Result<Size, Self::Error> {
            Ok(self.size)
        }

        fn window_size(&mut self) -> Result<WindowSize, Self::Error> {
            Ok(WindowSize {
                columns_rows: self.size,
                pixels: Size::new(0, 0),
            })
        }

        fn flush(&mut self) -> Result<(), Self::Error> {
            Ok(())
        }
    }

    #[test]
    fn terminal_surface_draw_uses_grid_diff_without_keycap_trailing_blank() {
        let (backend, output, _cursor_events) = CaptureBackend::new(4, 1);
        let mut surface = TerminalSurface::new(backend).unwrap();

        surface
            .draw(|area, buffer| {
                buffer.set_string(0, 0, "ab", Style::default());
                Some(area.as_position())
            })
            .unwrap();
        output.0.borrow_mut().clear();

        surface
            .draw(|area, buffer| {
                buffer.set_string(0, 0, "2️⃣", Style::default());
                Some(area.as_position())
            })
            .unwrap();

        let output = output.text();
        assert!(output.contains("2️⃣"));
        assert!(
            !output.contains("2️⃣ "),
            "wide keycap trailing cell must not be printed immediately after the grapheme: {output:?}"
        );
        assert!(
            !output.contains("\u{1b}[2G "),
            "wide keycap trailing cell must not be emitted as a standalone blank: {output:?}"
        );
    }

    #[test]
    fn terminal_surface_clears_placeholder_tail_when_keycap_replaces_it() {
        let (backend, output, _cursor_events) = CaptureBackend::new(16, 1);
        let mut surface = TerminalSurface::new(backend).unwrap();

        surface
            .draw(|_, buffer| {
                buffer.set_string(0, 0, "placeholder", Style::default());
                None
            })
            .unwrap();
        output.0.borrow_mut().clear();

        surface
            .draw(|_, buffer| {
                buffer.set_string(0, 0, "2️⃣", Style::default());
                None
            })
            .unwrap();

        let output = output.text();
        assert!(output.contains("2️⃣"));
        assert!(
            output.contains("\u{1b}[K"),
            "shorter keycap frame must clear previous placeholder tail: {output:?}"
        );
        assert!(
            !output.contains("2️⃣ "),
            "wide keycap trailing cell must not be printed while clearing placeholder tail: {output:?}"
        );
    }

    #[test]
    fn terminal_surface_preserves_skip_semantics_for_wide_keycap_tail() {
        let (backend, output, _cursor_events) = CaptureBackend::new(8, 1);
        let mut surface = TerminalSurface::new(backend).unwrap();

        surface
            .draw(|_, buffer| {
                buffer.set_string(
                    0,
                    0,
                    "2️⃣",
                    Style::default().add_modifier(Modifier::REVERSED),
                );
                buffer[(1, 0)].set_skip(true);
                None
            })
            .unwrap();

        let output = output.text();
        assert!(output.contains("2️⃣"));
        assert!(
            !output.contains("\u{1b}[2G "),
            "explicit skip tail must not be rendered as a standalone blank: {output:?}"
        );
    }

    #[test]
    fn terminal_surface_prefills_selected_keycap_width_before_printing_grapheme() {
        let (backend, output, _cursor_events) = CaptureBackend::new(8, 1);
        let mut surface = TerminalSurface::new(backend).unwrap();

        surface
            .draw(|_, buffer| {
                buffer.set_string(
                    0,
                    0,
                    "2️⃣",
                    Style::default().add_modifier(Modifier::REVERSED),
                );
                buffer[(1, 0)].set_style(Style::default().add_modifier(Modifier::REVERSED));
                None
            })
            .unwrap();

        let output = output.text();
        let reverse_index = output
            .find("\u{1b}[7m")
            .expect("selected keycap should enable reverse video");
        let prefill_index = output[reverse_index..]
            .find("  ")
            .map(|index| reverse_index + index)
            .expect("selected keycap should prefill both occupied columns");
        let keycap_index = output
            .find("2️⃣")
            .expect("selected keycap should still print the grapheme");
        assert!(
            prefill_index < keycap_index,
            "selection prefill must happen before printing the wide grapheme: {output:?}"
        );
        assert!(
            output[prefill_index..keycap_index].contains("\u{1b}[1;1H"),
            "renderer must move back to the grapheme start after prefilling its width: {output:?}"
        );
    }

    #[test]
    fn terminal_surface_prints_consecutive_keycaps_without_tail_blanks() {
        let (backend, output, _cursor_events) = CaptureBackend::new(10, 1);
        let mut surface = TerminalSurface::new(backend).unwrap();

        surface
            .draw(|_, buffer| {
                buffer.set_string(0, 0, "2️⃣2️⃣2️⃣", Style::default());
                None
            })
            .unwrap();

        let output = output.text();
        assert_eq!(output.matches("2️⃣").count(), 3, "{output:?}");
        assert!(
            output.contains("\u{1b}[1;3H") && output.contains("\u{1b}[1;5H"),
            "consecutive keycaps should keep explicit cell positioning between wide graphemes: {output:?}"
        );
        assert!(
            !output.contains("2️⃣ "),
            "consecutive keycaps must not render hidden tail blanks: {output:?}"
        );
    }

    #[test]
    fn terminal_surface_skips_repeated_hide_cursor_commands() {
        let (backend, _output, cursor_events) = CaptureBackend::new(4, 1);
        let mut surface = TerminalSurface::new(backend).unwrap();

        surface.draw(|_, _| None).unwrap();
        surface.draw(|_, _| None).unwrap();

        assert_eq!(&*cursor_events.borrow(), &[CursorEvent::Hide]);
    }

    #[test]
    fn terminal_surface_skips_repeated_show_cursor_commands() {
        let (backend, _output, cursor_events) = CaptureBackend::new(4, 1);
        let mut surface = TerminalSurface::new(backend).unwrap();
        surface.hide_cursor().unwrap();
        cursor_events.borrow_mut().clear();

        surface.draw(|area, _| Some(area.as_position())).unwrap();
        surface.draw(|area, _| Some(area.as_position())).unwrap();

        assert_eq!(&*cursor_events.borrow(), &[CursorEvent::Show]);
    }

    #[test]
    fn draw_commands_reapply_clear_background_after_attribute_reset() {
        let mut output = Vec::new();
        let mut first_cell = Cell::default();
        first_cell.set_symbol("a");
        first_cell.bg = Color::Blue;
        let mut second_cell = Cell::default();
        second_cell.set_symbol("b");
        second_cell.bg = Color::Blue;
        let commands = vec![
            TerminalDrawCommand::Put {
                x: 0,
                y: 0,
                cell: &first_cell,
                prefill_width: 0,
            },
            TerminalDrawCommand::ClearToEnd {
                x: 1,
                y: 0,
                bg: Color::Blue,
            },
            TerminalDrawCommand::Put {
                x: 0,
                y: 1,
                cell: &second_cell,
                prefill_width: 0,
            },
        ];

        draw_terminal_commands(&mut output, commands.into_iter()).unwrap();

        let output = String::from_utf8_lossy(&output);
        let mut expected_background = Vec::new();
        queue!(
            expected_background,
            SetBackgroundColor(Color::Blue.into_crossterm())
        )
        .unwrap();
        let expected_background = String::from_utf8_lossy(&expected_background);
        assert!(!expected_background.is_empty());
        let first_print_index = output.find('a').expect("first cell should be printed");
        let clear_index = output
            .find("\u{1b}[K")
            .expect("clear-to-end command should be emitted");
        let before_clear = &output[first_print_index + 'a'.len_utf8()..clear_index];
        assert!(
            before_clear.ends_with(expected_background.as_ref()),
            "clear should reapply the blue background after resetting attributes: {output:?}"
        );
    }

    #[test]
    fn draw_commands_emit_underline_color() {
        let mut output = Vec::new();
        let mut expected = Vec::new();
        queue!(expected, SetUnderlineColor(Color::Red.into_crossterm())).unwrap();
        assert!(!expected.is_empty());
        let mut cell = Cell::default();
        cell.set_symbol("u");
        cell.underline_color = Color::Red;
        cell.modifier = Modifier::UNDERLINED;

        draw_terminal_commands(
            &mut output,
            vec![TerminalDrawCommand::Put {
                x: 0,
                y: 0,
                cell: &cell,
                prefill_width: 0,
            }]
            .into_iter(),
        )
        .unwrap();

        let print_index = output
            .iter()
            .position(|byte| *byte == b'u')
            .expect("cell should be printed");
        let before_print = &output[..print_index];
        assert!(
            before_print
                .windows(expected.len())
                .any(|window| window == expected.as_slice()),
            "underline color should be forwarded to crossterm output: {:?}",
            String::from_utf8_lossy(&output)
        );
    }

    #[test]
    fn modifier_diff_reapplies_bold_when_dim_is_removed() {
        let mut output = Vec::new();
        let mut expected = Vec::new();
        queue!(
            expected,
            SetAttribute(CrosstermAttribute::NormalIntensity),
            SetAttribute(CrosstermAttribute::Bold)
        )
        .unwrap();

        ModifierDiff {
            from: Modifier::BOLD | Modifier::DIM,
            to: Modifier::BOLD,
        }
        .queue(&mut output)
        .unwrap();

        assert_eq!(output, expected);
    }
}
