use std::collections::HashSet;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PeerRate {
    pub id: u64,
    pub downloaded_from: u64,
    pub interested: bool,
}

pub fn select_unchoked(
    peers: &[PeerRate],
    regular_slots: usize,
    optimistic: Option<u64>,
) -> HashSet<u64> {
    let mut interested: Vec<&PeerRate> = peers.iter().filter(|p| p.interested).collect();
    interested.sort_by(|a, b| b.downloaded_from.cmp(&a.downloaded_from).then(a.id.cmp(&b.id)));
    let mut out: HashSet<u64> = interested
        .iter()
        .take(regular_slots)
        .map(|p| p.id)
        .collect();
    if let Some(id) = optimistic {
        if peers.iter().any(|p| p.id == id) {
            out.insert(id);
        }
    }
    out
}

#[derive(Debug, Clone)]
pub struct OptimisticRotor {
    current: Option<u64>,
    rotated_at: Option<Instant>,
    interval: Duration,
}

impl OptimisticRotor {
    pub fn new(interval: Duration) -> Self {
        OptimisticRotor {
            current: None,
            rotated_at: None,
            interval,
        }
    }

    pub fn current(&self) -> Option<u64> {
        self.current
    }

    pub fn maybe_rotate(&mut self, candidates: &[u64], now: Instant) -> Option<u64> {
        let due = match self.rotated_at {
            None => true,
            Some(at) => now.duration_since(at) >= self.interval,
        };
        let current_gone = match self.current {
            Some(id) => !candidates.contains(&id),
            None => true,
        };
        if !due && !current_gone {
            return self.current;
        }
        if candidates.is_empty() {
            self.current = None;
            return None;
        }
        let next = match self.current {
            Some(id) => {
                let position = candidates.iter().position(|c| *c == id);
                match position {
                    Some(i) => candidates[(i + 1) % candidates.len()],
                    None => candidates[0],
                }
            }
            None => candidates[0],
        };
        self.current = Some(next);
        self.rotated_at = Some(now);
        self.current
    }
}
