#[cfg(test)]
mod tests;

mod input;
mod render;
mod state;

pub(crate) use state::SessionPickerState;

#[cfg(test)]
use render::session_picker_meta_text_at;

const SESSION_PICKER_ROW_HEIGHT: usize = 4;
const SESSION_PICKER_PROMPT_MARKER_WIDTH: usize = 2;
const SESSION_PICKER_HEADER_HEIGHT: u16 = 1;
const SESSION_PICKER_HEADER_RULE_HEIGHT: u16 = 1;
const SESSION_PICKER_PAGE_RULE_HEIGHT: u16 = 1;
const SESSION_PICKER_FOOTER_HEIGHT: u16 = 1;
const SESSION_PICKER_CHROME_HEIGHT: u16 = SESSION_PICKER_HEADER_HEIGHT
    + SESSION_PICKER_HEADER_RULE_HEIGHT
    + SESSION_PICKER_PAGE_RULE_HEIGHT
    + SESSION_PICKER_FOOTER_HEIGHT;

fn session_picker_page_size_for_height(height: u16) -> usize {
    (usize::from(height.saturating_sub(SESSION_PICKER_CHROME_HEIGHT)) / SESSION_PICKER_ROW_HEIGHT)
        .max(1)
}
