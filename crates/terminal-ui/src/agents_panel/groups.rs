//! `/agents` 列表的行分组：Running / Just finished / Completed 三态、组内排序
//! 与显示顺序比较。分组是 `now_ms` 的纯函数，不存迁移状态——跨组迁移由消费方
//! （渲染 / 输入）在读取顺序敏感状态前重新计算。

use std::cmp::Ordering;

use runtime_domain::agent::{AgentOverviewRow, AgentProjectionStatus};

use crate::agents_panel::agent_status_is_running;

/// 终态行在 Just finished 过渡组的停留窗口；超过即归入 Completed。
pub(super) const AGENTS_JUST_FINISHED_WINDOW_MS: i64 = 10_000;

/// 组的三态；呈现顺序即枚举声明顺序（Running → Just finished → Completed）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AgentsRowGroupKind {
    Running,
    JustFinished,
    Completed,
}

/// 组全集：呈现顺序的唯一事实来源，同时核定页预算为组头行预留的最大行数。
pub(super) const AGENTS_ROW_GROUP_KINDS: [AgentsRowGroupKind; 3] = [
    AgentsRowGroupKind::Running,
    AgentsRowGroupKind::JustFinished,
    AgentsRowGroupKind::Completed,
];

impl AgentsRowGroupKind {
    /// 组头行的组名标签；组内计数由渲染层拼接。
    pub(super) fn header_label(self) -> &'static str {
        match self {
            Self::Running => "Running",
            Self::JustFinished => "Just finished",
            Self::Completed => "Completed",
        }
    }

    fn rank(self) -> u8 {
        match self {
            Self::Running => 0,
            Self::JustFinished => 1,
            Self::Completed => 2,
        }
    }
}

/// 一个非空分组：`row_indices` 指向输入行切片，已按组内排序规则排定。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AgentsRowGroup {
    pub(crate) kind: AgentsRowGroupKind,
    pub(crate) row_indices: Vec<usize>,
}

/// 行归属：非终态（含 CleanupBlocked）归 Running；终态按距 settled 的时长
/// 分流——窗口内是 Just finished，窗口外（含无计时起点的恢复行）归 Completed。
pub(super) fn agents_row_group_kind(row: &AgentOverviewRow, now_ms: i64) -> AgentsRowGroupKind {
    if agent_status_is_running(row.status) || row.status == AgentProjectionStatus::CleanupBlocked {
        return AgentsRowGroupKind::Running;
    }
    match row.settled_at_ms {
        Some(settled_at_ms)
            if now_ms.saturating_sub(settled_at_ms) < AGENTS_JUST_FINISHED_WINDOW_MS =>
        {
            AgentsRowGroupKind::JustFinished
        }
        _ => AgentsRowGroupKind::Completed,
    }
}

/// settled 时刻的组内排序键：无时刻视为最旧。Completed 升序时无时刻行排在
/// 最前；Just finished 取反序即新 settled 在前、无时刻行殿后。
fn settled_order_key(row: &AgentOverviewRow) -> (bool, i64) {
    (
        row.settled_at_ms.is_some(),
        row.settled_at_ms.unwrap_or(i64::MIN),
    )
}

/// 行的显示顺序比较：组间按 Running → Just finished → Completed；组内 Running
/// 按 agent_id 升序（CleanupBlocked 殿后）、Just finished 新 settled 在前、
/// Completed 旧 settled 在前（停留最久的行排在组首）。行存储排序、分页、
/// 导航与渲染共用该顺序。
pub(super) fn agents_row_display_order(
    a: &AgentOverviewRow,
    b: &AgentOverviewRow,
    now_ms: i64,
) -> Ordering {
    let kind_a = agents_row_group_kind(a, now_ms);
    let kind_b = agents_row_group_kind(b, now_ms);
    kind_a
        .rank()
        .cmp(&kind_b.rank())
        .then_with(|| match kind_a {
            AgentsRowGroupKind::Running => {
                let order_key = |row: &AgentOverviewRow| {
                    (
                        row.status == AgentProjectionStatus::CleanupBlocked,
                        row.agent_id.get(),
                    )
                };
                order_key(a).cmp(&order_key(b))
            }
            AgentsRowGroupKind::JustFinished => settled_order_key(b).cmp(&settled_order_key(a)),
            AgentsRowGroupKind::Completed => settled_order_key(a).cmp(&settled_order_key(b)),
        })
}

/// 分组纯函数：输入行引用切片 + 墙钟，输出按显示顺序排列的非空组序列
/// （`row_indices` 指向输入切片）。空组不产出。
pub(crate) fn agents_panel_row_groups(
    rows: &[&AgentOverviewRow],
    now_ms: i64,
) -> Vec<AgentsRowGroup> {
    let mut row_indices: Vec<usize> = (0..rows.len()).collect();
    row_indices.sort_by(|&a, &b| agents_row_display_order(rows[a], rows[b], now_ms));

    // 排序后同组行连续出现，分组即连续段切分。
    let mut groups: Vec<AgentsRowGroup> = Vec::new();
    for index in row_indices {
        let kind = agents_row_group_kind(rows[index], now_ms);
        match groups.last_mut() {
            Some(group) if group.kind == kind => group.row_indices.push(index),
            _ => groups.push(AgentsRowGroup {
                kind,
                row_indices: vec![index],
            }),
        }
    }
    groups
}

/// 分组判定的墙钟来源：共享的 Unix 毫秒时间戳。时钟异常退化为 0，
/// 只影响展示分组，不影响任何 runtime authority。
pub(super) fn agents_panel_now_unix_ms() -> i64 {
    runtime_domain::time::unix_timestamp_ms().unwrap_or(0)
}
