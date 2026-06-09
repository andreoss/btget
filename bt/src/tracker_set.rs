use crate::tracker::{http_announce, AnnounceRequest, AnnounceResponse, Error};
use crate::tracker_udp::udp_announce;
use std::time::{Duration, Instant};

pub type Announcer<'a> =
    &'a mut dyn FnMut(&str, &AnnounceRequest) -> Result<AnnounceResponse, Error>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackerSet {
    tiers: Vec<Vec<String>>,
}

impl TrackerSet {
    pub fn new(tiers: Vec<Vec<String>>) -> Self {
        TrackerSet { tiers }
    }

    pub fn tiers(&self) -> &[Vec<String>] {
        &self.tiers
    }

    pub fn announce(
        &mut self,
        request: &AnnounceRequest,
        announcer: Announcer,
    ) -> Result<AnnounceResponse, Error> {
        let mut last = Error::UnsupportedUrl("no trackers".to_string());
        for tier in self.tiers.iter_mut() {
            for index in 0..tier.len() {
                match announcer(&tier[index], request) {
                    Ok(response) => {
                        let url = tier.remove(index);
                        tier.insert(0, url);
                        return Ok(response);
                    }
                    Err(e) => last = e,
                }
            }
        }
        Err(last)
    }
}

pub fn announce_url(
    url: &str,
    request: &AnnounceRequest,
    timeout: Duration,
) -> Result<AnnounceResponse, Error> {
    if url.starts_with("udp://") {
        udp_announce(url, request, &[timeout, timeout * 2])
    } else {
        http_announce(url, request, timeout)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AnnounceTimer {
    due: Option<Instant>,
    failures: u32,
}

const BASE_BACKOFF: Duration = Duration::from_secs(15);
const MAX_BACKOFF: Duration = Duration::from_secs(900);

impl AnnounceTimer {
    pub fn new() -> Self {
        AnnounceTimer {
            due: None,
            failures: 0,
        }
    }

    pub fn ready(&self, now: Instant) -> bool {
        match self.due {
            None => true,
            Some(due) => now >= due,
        }
    }

    pub fn after_success(&mut self, interval_secs: u32, now: Instant) {
        self.failures = 0;
        self.due = Some(now + Duration::from_secs(interval_secs as u64));
    }

    pub fn after_failure(&mut self, now: Instant) {
        let backoff = BASE_BACKOFF
            .saturating_mul(2u32.saturating_pow(self.failures))
            .min(MAX_BACKOFF);
        self.failures = self.failures.saturating_add(1);
        self.due = Some(now + backoff);
    }
}

impl Default for AnnounceTimer {
    fn default() -> Self {
        Self::new()
    }
}
