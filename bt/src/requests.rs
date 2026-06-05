use std::time::{Duration, Instant};

pub const BLOCK_SIZE: u32 = 16384;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockRequest {
    pub piece: u32,
    pub begin: u32,
    pub length: u32,
}

pub fn piece_blocks(piece_length: u64) -> Vec<(u32, u32)> {
    let mut out = Vec::new();
    let mut begin = 0u64;
    while begin < piece_length {
        let length = std::cmp::min(BLOCK_SIZE as u64, piece_length - begin);
        out.push((begin as u32, length as u32));
        begin += length;
    }
    out
}

#[derive(Debug)]
pub struct RequestPipeline {
    capacity: usize,
    timeout: Duration,
    in_flight: Vec<(BlockRequest, Instant)>,
}

impl RequestPipeline {
    pub fn new(capacity: usize, timeout: Duration) -> Self {
        RequestPipeline {
            capacity,
            timeout,
            in_flight: Vec::new(),
        }
    }

    pub fn has_slot(&self) -> bool {
        self.in_flight.len() < self.capacity
    }

    pub fn in_flight(&self) -> usize {
        self.in_flight.len()
    }

    pub fn contains(&self, request: &BlockRequest) -> bool {
        self.in_flight.iter().any(|(r, _)| r == request)
    }

    pub fn issue(&mut self, request: BlockRequest, now: Instant) -> bool {
        if !self.has_slot() || self.contains(&request) {
            return false;
        }
        self.in_flight.push((request, now));
        true
    }

    pub fn complete(&mut self, piece: u32, begin: u32, length: u32) -> Option<BlockRequest> {
        let position = self
            .in_flight
            .iter()
            .position(|(r, _)| r.piece == piece && r.begin == begin && r.length == length)?;
        Some(self.in_flight.remove(position).0)
    }

    pub fn expired(&mut self, now: Instant) -> Vec<BlockRequest> {
        let timeout = self.timeout;
        let (dead, alive): (Vec<_>, Vec<_>) = self
            .in_flight
            .drain(..)
            .partition(|(_, issued)| now.duration_since(*issued) >= timeout);
        self.in_flight = alive;
        dead.into_iter().map(|(r, _)| r).collect()
    }

    pub fn drain(&mut self) -> Vec<BlockRequest> {
        self.in_flight.drain(..).map(|(r, _)| r).collect()
    }
}
