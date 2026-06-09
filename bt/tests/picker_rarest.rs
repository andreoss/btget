use bt::picker::PiecePicker;
use bt::pieces::{Availability, Bitfield};

fn bits(pieces: u32, set: &[u32]) -> Bitfield {
    let mut out = Bitfield::new(pieces);
    for index in set {
        out.set(*index);
    }
    out
}

fn availability(pieces: u32, swarm: &[&Bitfield]) -> Availability {
    let mut out = Availability::new(pieces);
    for member in swarm {
        out.add_bitfield(member);
    }
    out
}

#[test]
fn rarest_piece_picked_first() {
    let ours = Bitfield::new(4);
    let a = bits(4, &[0, 1, 2, 3]);
    let b = bits(4, &[0, 1, 2]);
    let c = bits(4, &[0, 1]);
    let avail = availability(4, &[&a, &b, &c]);
    let mut picker = PiecePicker::new(2);
    assert_eq!(picker.pick(&ours, &a, &avail), Some(3));
    assert_eq!(picker.pick(&ours, &a, &avail), Some(2));
    assert_eq!(picker.pick(&ours, &a, &avail), Some(0));
    assert_eq!(picker.pick(&ours, &a, &avail), Some(1));
}

#[test]
fn only_peer_pieces_are_picked() {
    let ours = Bitfield::new(4);
    let peer = bits(4, &[1]);
    let avail = availability(4, &[&peer]);
    let mut picker = PiecePicker::new(2);
    assert_eq!(picker.pick(&ours, &peer, &avail), Some(1));
    assert_eq!(picker.pick(&ours, &peer, &avail), Some(1));
    assert_eq!(picker.pick(&ours, &peer, &avail), None);
}

#[test]
fn owned_pieces_not_picked() {
    let ours = bits(3, &[0, 2]);
    let peer = bits(3, &[0, 1, 2]);
    let avail = availability(3, &[&peer]);
    let mut picker = PiecePicker::new(1);
    assert_eq!(picker.pick(&ours, &peer, &avail), Some(1));
    assert_eq!(picker.pick(&ours, &peer, &avail), None);
}

#[test]
fn endgame_duplicates_up_to_cap() {
    let ours = Bitfield::new(2);
    let peer = bits(2, &[0, 1]);
    let avail = availability(2, &[&peer]);
    let mut picker = PiecePicker::new(3);
    assert_eq!(picker.pick(&ours, &peer, &avail), Some(0));
    assert_eq!(picker.pick(&ours, &peer, &avail), Some(1));
    assert!(picker.in_endgame(&ours));
    let mut counts = std::collections::HashMap::new();
    while let Some(piece) = picker.pick(&ours, &peer, &avail) {
        *counts.entry(piece).or_insert(0u32) += 1;
    }
    assert_eq!(picker.active_count(0), 3);
    assert_eq!(picker.active_count(1), 3);
    assert_eq!(counts.get(&0), Some(&2));
    assert_eq!(counts.get(&1), Some(&2));
}

#[test]
fn complete_and_abandon_release_slots() {
    let ours = Bitfield::new(2);
    let peer = bits(2, &[0, 1]);
    let avail = availability(2, &[&peer]);
    let mut picker = PiecePicker::new(1);
    assert_eq!(picker.pick(&ours, &peer, &avail), Some(0));
    assert_eq!(picker.pick(&ours, &peer, &avail), Some(1));
    assert_eq!(picker.pick(&ours, &peer, &avail), None);
    picker.abandon(1);
    assert_eq!(picker.pick(&ours, &peer, &avail), Some(1));
    picker.complete(0);
    let ours = bits(2, &[0]);
    assert_eq!(picker.pick(&ours, &peer, &avail), None);
    assert!(picker.in_endgame(&ours));
}

#[test]
fn simulated_swarm_downloads_rarest_order() {
    let pieces = 8u32;
    let ours = Bitfield::new(pieces);
    let seeds = [
        bits(pieces, &[0, 1, 2, 3, 4, 5, 6, 7]),
        bits(pieces, &[0, 1, 2, 3, 4, 5, 6]),
        bits(pieces, &[0, 1, 2, 3, 4, 5]),
        bits(pieces, &[0, 1, 2, 3]),
    ];
    let avail = availability(pieces, &[&seeds[0], &seeds[1], &seeds[2], &seeds[3]]);
    let mut picker = PiecePicker::new(1);
    let mut order = Vec::new();
    let full = &seeds[0];
    while let Some(piece) = picker.pick(&ours, full, &avail) {
        order.push(piece);
    }
    assert_eq!(order, vec![7, 6, 4, 5, 0, 1, 2, 3]);
}
