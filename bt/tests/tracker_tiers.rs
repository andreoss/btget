use bt::metainfo::InfoHash;
use bt::tracker::{AnnounceRequest, AnnounceResponse, Error, Event};
use bt::tracker_set::{AnnounceTimer, TrackerSet};
use std::time::{Duration, Instant};

fn request() -> AnnounceRequest {
    AnnounceRequest {
        info_hash: InfoHash([1; 20]),
        peer_id: *b"-BG0001-abcdefghijkl",
        port: 6881,
        uploaded: 0,
        downloaded: 0,
        left: 1,
        event: Event::None,
    }
}

fn ok_response() -> AnnounceResponse {
    AnnounceResponse {
        interval: 60,
        peers: vec![],
    }
}

#[test]
fn first_working_url_wins_and_is_promoted() {
    let mut set = TrackerSet::new(vec![vec![
        "http://a/announce".to_string(),
        "http://b/announce".to_string(),
    ]]);
    let mut calls = Vec::new();
    let result = set.announce(&request(), &mut |url, _| {
        calls.push(url.to_string());
        if url.contains("//a/") {
            Err(Error::Io("down".to_string()))
        } else {
            Ok(ok_response())
        }
    });
    assert!(result.is_ok());
    assert_eq!(calls, vec!["http://a/announce", "http://b/announce"]);
    assert_eq!(
        set.tiers()[0],
        vec!["http://b/announce".to_string(), "http://a/announce".to_string()]
    );
    let mut calls = Vec::new();
    let _ = set.announce(&request(), &mut |url, _| {
        calls.push(url.to_string());
        Ok(ok_response())
    });
    assert_eq!(calls, vec!["http://b/announce"]);
}

#[test]
fn second_tier_used_when_first_fails() {
    let mut set = TrackerSet::new(vec![
        vec!["udp://t1:1".to_string()],
        vec!["udp://t2:1".to_string()],
    ]);
    let result = set.announce(&request(), &mut |url, _| {
        if url.contains("t1") {
            Err(Error::Io("down".to_string()))
        } else {
            Ok(ok_response())
        }
    });
    assert!(result.is_ok());
}

#[test]
fn all_failing_returns_last_error() {
    let mut set = TrackerSet::new(vec![vec!["udp://t1:1".to_string()]]);
    let result = set.announce(&request(), &mut |_, _| Err(Error::HttpStatus(500)));
    assert_eq!(result, Err(Error::HttpStatus(500)));
}

#[test]
fn empty_set_errors() {
    let mut set = TrackerSet::new(vec![]);
    assert!(set.announce(&request(), &mut |_, _| Ok(ok_response())).is_err());
}

#[test]
fn timer_starts_ready() {
    let timer = AnnounceTimer::new();
    assert!(timer.ready(Instant::now()));
}

#[test]
fn timer_waits_for_interval_after_success() {
    let mut timer = AnnounceTimer::new();
    let now = Instant::now();
    timer.after_success(60, now);
    assert!(!timer.ready(now + Duration::from_secs(59)));
    assert!(timer.ready(now + Duration::from_secs(60)));
}

#[test]
fn timer_backoff_doubles_and_caps() {
    let mut timer = AnnounceTimer::new();
    let now = Instant::now();
    timer.after_failure(now);
    assert!(!timer.ready(now + Duration::from_secs(14)));
    assert!(timer.ready(now + Duration::from_secs(15)));
    timer.after_failure(now);
    assert!(!timer.ready(now + Duration::from_secs(29)));
    assert!(timer.ready(now + Duration::from_secs(30)));
    for _ in 0..20 {
        timer.after_failure(now);
    }
    assert!(timer.ready(now + Duration::from_secs(900)));
}

#[test]
fn success_resets_backoff() {
    let mut timer = AnnounceTimer::new();
    let now = Instant::now();
    timer.after_failure(now);
    timer.after_failure(now);
    timer.after_success(10, now);
    timer.after_failure(now);
    assert!(timer.ready(now + Duration::from_secs(15)));
    assert!(!timer.ready(now + Duration::from_secs(14)));
}
