use bt::choke::{select_unchoked, OptimisticRotor, PeerRate};
use std::time::{Duration, Instant};

fn rate(id: u64, downloaded: u64, interested: bool) -> PeerRate {
    PeerRate {
        id,
        downloaded_from: downloaded,
        interested,
    }
}

#[test]
fn top_downloaders_win_slots() {
    let peers = vec![
        rate(1, 100, true),
        rate(2, 300, true),
        rate(3, 200, true),
        rate(4, 400, true),
    ];
    let unchoked = select_unchoked(&peers, 3, None);
    assert!(unchoked.contains(&4) && unchoked.contains(&2) && unchoked.contains(&3));
    assert!(!unchoked.contains(&1));
}

#[test]
fn uninterested_peers_get_no_slot() {
    let peers = vec![rate(1, 500, false), rate(2, 10, true)];
    let unchoked = select_unchoked(&peers, 2, None);
    assert_eq!(unchoked.len(), 1);
    assert!(unchoked.contains(&2));
}

#[test]
fn optimistic_peer_added_on_top() {
    let peers = vec![rate(1, 100, true), rate(2, 50, true), rate(3, 0, true)];
    let unchoked = select_unchoked(&peers, 1, Some(3));
    assert!(unchoked.contains(&1));
    assert!(unchoked.contains(&3));
    assert_eq!(unchoked.len(), 2);
}

#[test]
fn vanished_optimistic_ignored() {
    let peers = vec![rate(1, 100, true)];
    let unchoked = select_unchoked(&peers, 1, Some(9));
    assert_eq!(unchoked.len(), 1);
}

#[test]
fn rotor_holds_between_intervals() {
    let mut rotor = OptimisticRotor::new(Duration::from_secs(30));
    let now = Instant::now();
    assert_eq!(rotor.maybe_rotate(&[5, 6, 7], now), Some(5));
    assert_eq!(
        rotor.maybe_rotate(&[5, 6, 7], now + Duration::from_secs(10)),
        Some(5)
    );
    assert_eq!(
        rotor.maybe_rotate(&[5, 6, 7], now + Duration::from_secs(30)),
        Some(6)
    );
}

#[test]
fn rotor_round_robins() {
    let mut rotor = OptimisticRotor::new(Duration::from_secs(0));
    let now = Instant::now();
    assert_eq!(rotor.maybe_rotate(&[1, 2], now), Some(1));
    assert_eq!(rotor.maybe_rotate(&[1, 2], now), Some(2));
    assert_eq!(rotor.maybe_rotate(&[1, 2], now), Some(1));
}

#[test]
fn rotor_replaces_vanished_current() {
    let mut rotor = OptimisticRotor::new(Duration::from_secs(600));
    let now = Instant::now();
    assert_eq!(rotor.maybe_rotate(&[1, 2], now), Some(1));
    assert_eq!(rotor.maybe_rotate(&[2, 3], now), Some(2));
}

#[test]
fn rotor_empties_gracefully() {
    let mut rotor = OptimisticRotor::new(Duration::from_secs(0));
    let now = Instant::now();
    assert_eq!(rotor.maybe_rotate(&[], now), None);
    assert_eq!(rotor.current(), None);
}
