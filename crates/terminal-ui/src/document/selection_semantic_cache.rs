use std::rc::Rc;

use crate::{bounded_lru_cache::BoundedLruCache, selection::SelectableLineRange};

pub(super) const MAX_SELECTION_SEMANTIC_ITEMS: usize = 32;

#[derive(Debug)]
pub(super) struct SelectionSemanticEntry {
    pub(super) plain_lines: Vec<String>,
    pub(super) selectable_ranges: Vec<SelectableLineRange>,
}

#[derive(Debug)]
pub(super) struct SelectionSemanticCache {
    entries: BoundedLruCache<usize, Rc<SelectionSemanticEntry>>,
}

impl Default for SelectionSemanticCache {
    fn default() -> Self {
        Self {
            entries: BoundedLruCache::new(MAX_SELECTION_SEMANTIC_ITEMS),
        }
    }
}

impl SelectionSemanticCache {
    pub(super) fn get_or_insert_with(
        &mut self,
        item_index: usize,
        build: impl FnOnce() -> SelectionSemanticEntry,
    ) -> Rc<SelectionSemanticEntry> {
        if let Some(entry) = self.entries.get_cloned(&item_index) {
            return entry;
        }

        let entry = Rc::new(build());
        self.entries.insert(item_index, Rc::clone(&entry));
        entry
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
        self.entries.peek_cloned(&item_index)
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
        }
        cache
    }
}
