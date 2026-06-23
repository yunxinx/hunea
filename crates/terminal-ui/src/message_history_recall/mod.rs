//! 盲回溯（空输入或仍匹配上次 recall 时的 Up/Down）状态机。

mod state;

#[cfg(test)]
mod tests;

pub(crate) use state::{BlindRecallNavigateResult, BlindRecallState};
