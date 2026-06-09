use crate::pieces::{Availability, Bitfield};
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct PiecePicker {
    active: HashMap<u32, u32>,
    endgame_cap: u32,
}

impl PiecePicker {
    pub fn new(endgame_cap: u32) -> Self {
        PiecePicker {
            active: HashMap::new(),
            endgame_cap,
        }
    }

    pub fn active_count(&self, piece: u32) -> u32 {
        self.active.get(&piece).copied().unwrap_or(0)
    }

    pub fn pick(
        &mut self,
        ours: &Bitfield,
        theirs: &Bitfield,
        availability: &Availability,
    ) -> Option<u32> {
        let candidates: Vec<u32> = ours
            .missing()
            .filter(|index| theirs.has(*index))
            .collect();
        let fresh = candidates
            .iter()
            .copied()
            .filter(|index| !self.active.contains_key(index))
            .min_by_key(|index| (availability.count(*index), *index));
        let chosen = match fresh {
            Some(index) => Some(index),
            None => candidates
                .iter()
                .copied()
                .filter(|index| self.active_count(*index) < self.endgame_cap)
                .min_by_key(|index| {
                    (self.active_count(*index), availability.count(*index), *index)
                }),
        }?;
        *self.active.entry(chosen).or_insert(0) += 1;
        Some(chosen)
    }

    pub fn complete(&mut self, piece: u32) {
        self.active.remove(&piece);
    }

    pub fn abandon(&mut self, piece: u32) {
        if let Some(count) = self.active.get_mut(&piece) {
            *count -= 1;
            if *count == 0 {
                self.active.remove(&piece);
            }
        }
    }

    pub fn in_endgame(&self, ours: &Bitfield) -> bool {
        ours.missing().all(|index| self.active.contains_key(&index))
    }
}
