use crate::peer::Message;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PeerState {
    pub am_choking: bool,
    pub am_interested: bool,
    pub peer_choking: bool,
    pub peer_interested: bool,
}

impl Default for PeerState {
    fn default() -> Self {
        PeerState {
            am_choking: true,
            am_interested: false,
            peer_choking: true,
            peer_interested: false,
        }
    }
}

impl PeerState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn on_message(&mut self, message: &Message) {
        match message {
            Message::Choke => self.peer_choking = true,
            Message::Unchoke => self.peer_choking = false,
            Message::Interested => self.peer_interested = true,
            Message::NotInterested => self.peer_interested = false,
            _ => {}
        }
    }

    pub fn set_interested(&mut self, interested: bool) {
        self.am_interested = interested;
    }

    pub fn set_choking(&mut self, choking: bool) {
        self.am_choking = choking;
    }

    pub fn can_request(&self) -> bool {
        self.am_interested && !self.peer_choking
    }

    pub fn may_upload(&self) -> bool {
        !self.am_choking && self.peer_interested
    }
}
