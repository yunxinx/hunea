//! 盲回溯（空输入或仍匹配上次 recall 时的 Up/Down）状态机。

mod commit;
mod state;

#[cfg(test)]
mod tests;

pub(crate) use commit::{
    commit_message_history, message_history_record_effect, stage_message_history_recall,
};
pub(crate) use state::BlindRecallState;
