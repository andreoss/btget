use sha1::{Digest, Sha1};
use std::cell::Cell;
use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};

pub struct Source {
    state: RandomState,
    seed: u64,
    counter: Cell<u64>,
}

impl Source {
    pub fn new() -> Self {
        let state = RandomState::new();
        let mut hasher = state.build_hasher();
        hasher.write_u64(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos() as u64)
                .unwrap_or(0),
        );
        hasher.write_u32(std::process::id());
        Source {
            seed: hasher.finish(),
            state,
            counter: Cell::new(0),
        }
    }

    pub fn next_u64(&self) -> u64 {
        let counter = self.counter.get().wrapping_add(1);
        self.counter.set(counter);
        let mut hasher = self.state.build_hasher();
        hasher.write_u64(self.seed);
        hasher.write_u64(counter);
        hasher.finish()
    }

    pub fn fill(&self, out: &mut [u8]) {
        for chunk in out.chunks_mut(20) {
            let digest = Sha1::digest(self.next_u64().to_be_bytes());
            chunk.copy_from_slice(&digest[..chunk.len()]);
        }
    }
}

impl Default for Source {
    fn default() -> Self {
        Self::new()
    }
}
