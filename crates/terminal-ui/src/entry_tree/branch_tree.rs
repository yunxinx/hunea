use std::collections::{HashMap, HashSet};

use runtime_domain::session::SessionBranchTreeNode;

pub(super) fn branch_tree_display_order_nodes(
    nodes: Vec<SessionBranchTreeNode>,
) -> Vec<SessionBranchTreeNode> {
    let children_by_parent = branch_tree_children_by_parent(&nodes);
    let mut ordered_indices = Vec::with_capacity(nodes.len());
    let mut visited = HashSet::new();

    branch_tree_push_ordered_child_indices(
        None,
        &nodes,
        &children_by_parent,
        &mut visited,
        &mut ordered_indices,
    );
    for index in 0..nodes.len() {
        if visited.insert(index) {
            ordered_indices.push(index);
        }
    }

    let mut nodes_by_index = nodes.into_iter().map(Some).collect::<Vec<_>>();
    ordered_indices
        .into_iter()
        .filter_map(|index| nodes_by_index[index].take())
        .collect()
}

fn branch_tree_push_ordered_child_indices(
    parent_branch_row_id: Option<&str>,
    nodes: &[SessionBranchTreeNode],
    children_by_parent: &HashMap<Option<String>, Vec<usize>>,
    visited: &mut HashSet<usize>,
    ordered_indices: &mut Vec<usize>,
) {
    let key = parent_branch_row_id.map(str::to_string);
    let Some(children) = children_by_parent.get(&key) else {
        return;
    };
    for child_index in children.iter().copied() {
        if !visited.insert(child_index) {
            continue;
        }
        ordered_indices.push(child_index);
        branch_tree_push_ordered_child_indices(
            Some(&nodes[child_index].branch.branch_row_id),
            nodes,
            children_by_parent,
            visited,
            ordered_indices,
        );
    }
}

fn branch_tree_children_by_parent(
    nodes: &[SessionBranchTreeNode],
) -> HashMap<Option<String>, Vec<usize>> {
    let mut children_by_parent: HashMap<Option<String>, Vec<usize>> = HashMap::new();
    for (index, node) in nodes.iter().enumerate() {
        children_by_parent
            .entry(node.parent_branch_row_id.clone())
            .or_default()
            .push(index);
    }
    children_by_parent
}

pub(super) fn branch_tree_connector_prefixes(nodes: &[SessionBranchTreeNode]) -> Vec<String> {
    let children_by_parent = branch_tree_children_by_parent(nodes);
    let mut prefixes = vec![String::new(); nodes.len()];
    let mut ancestor_has_next_sibling = Vec::new();
    let mut visited = HashSet::new();

    branch_tree_fill_connector_prefixes(
        None,
        nodes,
        &children_by_parent,
        &mut ancestor_has_next_sibling,
        &mut visited,
        &mut prefixes,
    );
    for (index, prefix) in prefixes.iter_mut().enumerate() {
        if visited.insert(index) {
            *prefix = "└── ".to_string();
        }
    }

    prefixes
}

fn branch_tree_fill_connector_prefixes(
    parent_branch_row_id: Option<&str>,
    nodes: &[SessionBranchTreeNode],
    children_by_parent: &HashMap<Option<String>, Vec<usize>>,
    ancestor_has_next_sibling: &mut Vec<bool>,
    visited: &mut HashSet<usize>,
    prefixes: &mut [String],
) {
    let key = parent_branch_row_id.map(str::to_string);
    let Some(children) = children_by_parent.get(&key) else {
        return;
    };
    for (position, child_index) in children.iter().copied().enumerate() {
        if !visited.insert(child_index) {
            continue;
        }
        let is_last = position + 1 == children.len();
        let mut prefix = String::new();
        for has_next_sibling in ancestor_has_next_sibling.iter().copied() {
            if has_next_sibling {
                prefix.push_str("│   ");
            } else {
                prefix.push_str("    ");
            }
        }
        prefix.push_str(if is_last { "└── " } else { "├── " });
        prefixes[child_index] = prefix;

        ancestor_has_next_sibling.push(!is_last);
        branch_tree_fill_connector_prefixes(
            Some(&nodes[child_index].branch.branch_row_id),
            nodes,
            children_by_parent,
            ancestor_has_next_sibling,
            visited,
            prefixes,
        );
        ancestor_has_next_sibling.pop();
    }
}
