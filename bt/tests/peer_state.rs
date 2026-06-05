use bt::peer::Message;
use bt::peer_state::PeerState;

#[test]
fn initial_state_is_choked_and_uninterested() {
    let state = PeerState::new();
    assert!(state.am_choking);
    assert!(state.peer_choking);
    assert!(!state.am_interested);
    assert!(!state.peer_interested);
    assert!(!state.can_request());
    assert!(!state.may_upload());
}

#[test]
fn peer_messages_flip_their_flags() {
    let mut state = PeerState::new();
    state.on_message(&Message::Unchoke);
    assert!(!state.peer_choking);
    state.on_message(&Message::Choke);
    assert!(state.peer_choking);
    state.on_message(&Message::Interested);
    assert!(state.peer_interested);
    state.on_message(&Message::NotInterested);
    assert!(!state.peer_interested);
}

#[test]
fn unrelated_messages_leave_state_alone() {
    let mut state = PeerState::new();
    state.on_message(&Message::Have(3));
    state.on_message(&Message::Bitfield(vec![0xff]));
    state.on_message(&Message::KeepAlive);
    assert_eq!(state, PeerState::new());
}

#[test]
fn request_gate_needs_interest_and_unchoke() {
    let mut state = PeerState::new();
    state.set_interested(true);
    assert!(!state.can_request());
    state.on_message(&Message::Unchoke);
    assert!(state.can_request());
    state.set_interested(false);
    assert!(!state.can_request());
}

#[test]
fn upload_gate_needs_their_interest_and_our_unchoke() {
    let mut state = PeerState::new();
    state.on_message(&Message::Interested);
    assert!(!state.may_upload());
    state.set_choking(false);
    assert!(state.may_upload());
    state.on_message(&Message::NotInterested);
    assert!(!state.may_upload());
}
