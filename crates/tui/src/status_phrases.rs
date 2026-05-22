use std::time::{SystemTime, UNIX_EPOCH};

use mo_core::phrases::{DEFAULT_STATUS_PHRASES, StatusPhraseOrder};

/// `StatusPhraseSelector` 负责为每个新请求挑选等待行 fallback 文案。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct StatusPhraseSelector {
    phrases: Vec<String>,
    order: StatusPhraseOrder,
    next_cycle_index: usize,
    random_counter: u64,
}

impl StatusPhraseSelector {
    pub(super) fn new(phrases: Vec<String>, order: StatusPhraseOrder) -> Self {
        let phrases = normalize_phrases(phrases);
        Self {
            phrases,
            order,
            next_cycle_index: 0,
            random_counter: 0,
        }
    }

    pub(super) fn next_phrase(&mut self) -> String {
        if self.phrases.is_empty() {
            return "Generating".to_string();
        }

        match self.order {
            StatusPhraseOrder::Cycle => self.next_cycle_phrase(),
            StatusPhraseOrder::Random => self.next_random_phrase(),
        }
    }

    fn next_cycle_phrase(&mut self) -> String {
        let index = self.next_cycle_index % self.phrases.len();
        self.next_cycle_index = self.next_cycle_index.wrapping_add(1);
        self.phrases[index].clone()
    }

    fn next_random_phrase(&mut self) -> String {
        self.random_counter = self.random_counter.wrapping_add(1);
        let index = randomish_index(self.phrases.len(), self.random_counter);
        self.phrases[index].clone()
    }
}

impl Default for StatusPhraseSelector {
    fn default() -> Self {
        Self::new(default_status_phrases(), StatusPhraseOrder::Random)
    }
}

pub(super) fn default_status_phrases() -> Vec<String> {
    DEFAULT_STATUS_PHRASES
        .iter()
        .map(|phrase| (*phrase).to_string())
        .collect()
}

fn normalize_phrases(phrases: Vec<String>) -> Vec<String> {
    phrases
        .into_iter()
        .map(|phrase| phrase.trim().to_string())
        .filter(|phrase| !phrase.is_empty())
        .collect()
}

fn randomish_index(len: usize, counter: u64) -> usize {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let mut seed =
        (nanos as u64) ^ ((nanos >> 64) as u64) ^ counter.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    seed ^= seed >> 30;
    seed = seed.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    seed ^= seed >> 27;
    seed = seed.wrapping_mul(0x94D0_49BB_1331_11EB);
    seed ^= seed >> 31;
    (seed as usize) % len
}
