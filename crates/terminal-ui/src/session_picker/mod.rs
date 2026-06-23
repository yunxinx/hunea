#[cfg(test)]
mod tests;

mod input;
mod render;
mod state;

pub(crate) use state::SessionPickerState;

#[cfg(test)]
use render::session_picker_meta_text_at;

use crate::fullscreen_list_chrome::fullscreen_list_page_size_for_height_with_row_height;

const SESSION_PICKER_ROW_HEIGHT: usize = 4;
const SESSION_PICKER_PROMPT_MARKER_WIDTH: usize = 2;

fn session_picker_page_size_for_height(height: u16) -> usize {
    fullscreen_list_page_size_for_height_with_row_height(height, SESSION_PICKER_ROW_HEIGHT)
}
