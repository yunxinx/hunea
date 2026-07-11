use std::{
    collections::{HashMap, VecDeque},
    rc::Rc,
};

use crate::selection::SelectableLineRange;

pub(super) const MAX_SELECTION_SEMANTIC_ITEMS: usize = 32;

#[derive(Debug)]
pub(super) struct SelectionSemanticEntry {
    pub(super) plain_lines: Vec<String>,
    pub(super) selectable_ranges: Vec<SelectableLineRange>,
}

#[derive(Debug, Default)]
pub(super) struct SelectionSemanticCache {
    entries: HashMap<usize, Rc<SelectionSemanticEntry>>,
    recent: VecDeque<usize>,
}

impl SelectionSemanticCache {
    pub(super) fn get_or_insert_with(
        &mut self,
        item_index: usize,
        build: impl FnOnce() -> SelectionSemanticEntry,
    ) -> Rc<SelectionSemanticEntry> {
        if let Some(entry) = self.entries.get(&item_index).cloned() {
            self.touch(item_index);
            return entry;
        }

        let entry = Rc::new(build());
        self.entries.insert(item_index, Rc::clone(&entry));
        self.touch(item_index);
        while self.entries.len() > MAX_SELECTION_SEMANTIC_ITEMS {
            let Some(oldest) = self.recent.pop_front() else {
                break;
            };
            self.entries.remove(&oldest);
        }
        entry
    }

    fn touch(&mut self, item_index: usize) {
        if let Some(position) = self.recent.iter().position(|index| *index == item_index) {
            self.recent.remove(position);
        }
        self.recent.push_back(item_index);
    }

    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        self.entries.len()
    }

    #[cfg(test)]
    pub(super) fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    #[cfg(test)]
    pub(super) fn get(&self, item_index: usize) -> Option<Rc<SelectionSemanticEntry>> {
        self.entries.get(&item_index).cloned()
    }

    #[cfg(test)]
    pub(super) fn from_plain_lines(lines: impl IntoIterator<Item = (usize, Vec<String>)>) -> Self {
        let mut cache = Self::default();
        for (item_index, plain_lines) in lines {
            cache.entries.insert(
                item_index,
                Rc::new(SelectionSemanticEntry {
                    selectable_ranges: vec![SelectableLineRange::default(); plain_lines.len()],
                    plain_lines,
                }),
            );
            cache.recent.push_back(item_index);
        }
        cache
    }
}
