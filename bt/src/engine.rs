use crate::choke::{select_unchoked, OptimisticRotor, PeerRate};
use crate::metainfo::Metainfo;
use crate::peer::{exchange_handshake, read_message, write_message, Handshake, Message};
use crate::peer_state::PeerState;
use crate::picker::PiecePicker;
use crate::pieces::{Availability, Bitfield};
use crate::requests::{piece_blocks, BlockRequest, RequestPipeline};
use crate::storage::Storage;
use crate::tracker::{AnnounceRequest, Event as TrackerEvent};
use crate::tracker_set::{announce_url, AnnounceTimer, TrackerSet};
use crate::verify::verify_piece;
use std::collections::{HashMap, HashSet};
use std::net::{Shutdown, SocketAddr, TcpStream};
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct EngineConfig {
    pub output_dir: PathBuf,
    pub peer_id: [u8; 20],
    pub port: u16,
    pub max_peers: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    Announced { peers: usize },
    Connected { addr: SocketAddr },
    Peers { count: usize },
    PieceDone { index: u32, have: u32, total: u32 },
    Rechecking { bad: u32 },
    Complete,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    NoTrackers,
    NoPeers,
    Io(String),
    Incomplete,
}

const ANNOUNCE_TIMEOUT: Duration = Duration::from_secs(20);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const READ_TIMEOUT: Duration = Duration::from_secs(120);
const PIPELINE_DEPTH: usize = 32;
const BLOCK_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_EMPTY_ROUNDS: usize = 5;
const MAX_VERIFY_FAILURES: u32 = 2;
const ENDGAME_DUP: u32 = 2;
const REGULAR_UNCHOKE_SLOTS: usize = 3;
const CHOKE_INTERVAL: Duration = Duration::from_secs(10);
const OPTIMISTIC_INTERVAL: Duration = Duration::from_secs(30);
const SNUB_TIMEOUT: Duration = Duration::from_secs(90);
const MAX_SERVED_BLOCK: u32 = 1 << 17;

enum In {
    Connected {
        id: u64,
        addr: SocketAddr,
        tx: Sender<Message>,
        stream: TcpStream,
    },
    Msg {
        id: u64,
        message: Message,
    },
    Died {
        id: u64,
    },
    ConnectFailed,
}

struct PieceJob {
    index: u32,
    data: Vec<u8>,
    received: u64,
    pending: Vec<(u32, u32)>,
    failures: u32,
}

impl PieceJob {
    fn start(index: u32, size: u64) -> Self {
        PieceJob {
            index,
            data: vec![0u8; size as usize],
            received: 0,
            pending: piece_blocks(size),
            failures: 0,
        }
    }

    fn restart(&mut self) {
        self.received = 0;
        self.pending = piece_blocks(self.data.len() as u64);
        self.failures += 1;
    }

    fn done(&self) -> bool {
        self.received == self.data.len() as u64
    }
}

struct Peer {
    tx: Sender<Message>,
    stream: TcpStream,
    state: PeerState,
    theirs: Bitfield,
    theirs_known: bool,
    job: Option<PieceJob>,
    pipeline: RequestPipeline,
    downloaded_from: u64,
    last_useful: Instant,
}

struct Engine<'a> {
    meta: &'a Metainfo,
    config: &'a EngineConfig,
    storage: Storage,
    ours: Bitfield,
    availability: Availability,
    picker: PiecePicker,
    peers: HashMap<u64, Peer>,
    queue: Vec<SocketAddr>,
    known: HashSet<SocketAddr>,
    connecting: usize,
    downloaded: u64,
    uploaded: u64,
    trackers: TrackerSet,
    timer: AnnounceTimer,
    rotor: OptimisticRotor,
    last_choke: Instant,
    empty_rounds: usize,
    tx_in: Sender<In>,
}

pub fn download(
    meta: &Metainfo,
    config: &EngineConfig,
    on_event: &mut dyn FnMut(&Event),
) -> Result<(), Error> {
    if meta.trackers.is_empty() {
        return Err(Error::NoTrackers);
    }
    let storage = Storage::from_metainfo(meta, &config.output_dir);
    storage.allocate().map_err(|e| Error::Io(e.to_string()))?;
    let total_pieces = meta.pieces.len() as u32;
    let (tx_in, rx_in) = channel();
    let mut engine = Engine {
        meta,
        config,
        storage,
        ours: Bitfield::new(total_pieces),
        availability: Availability::new(total_pieces),
        picker: PiecePicker::new(ENDGAME_DUP),
        peers: HashMap::new(),
        queue: Vec::new(),
        known: HashSet::new(),
        connecting: 0,
        downloaded: 0,
        uploaded: 0,
        trackers: TrackerSet::new(meta.trackers.clone()),
        timer: AnnounceTimer::new(),
        rotor: OptimisticRotor::new(OPTIMISTIC_INTERVAL),
        last_choke: Instant::now() - CHOKE_INTERVAL,
        empty_rounds: 0,
        tx_in,
    };
    let mut rechecks = 0;
    let result = loop {
        match engine.run(&rx_in, on_event) {
            Ok(()) => {
                let bad = engine.final_check();
                if bad == 0 {
                    break Ok(());
                }
                on_event(&Event::Rechecking { bad });
                rechecks += 1;
                if rechecks > 2 {
                    break Err(Error::Incomplete);
                }
            }
            Err(e) => break Err(e),
        }
    };
    for (_, peer) in engine.peers.drain() {
        let _ = peer.stream.shutdown(Shutdown::Both);
    }
    match &result {
        Ok(()) => {
            let _ = engine.announce(TrackerEvent::Completed);
            on_event(&Event::Complete);
        }
        Err(_) => {
            let _ = engine.announce(TrackerEvent::Stopped);
        }
    }
    result
}

impl<'a> Engine<'a> {
    fn run(
        &mut self,
        rx_in: &Receiver<In>,
        on_event: &mut dyn FnMut(&Event),
    ) -> Result<(), Error> {
        let mut next_id = 0u64;
        loop {
            if self.ours.is_complete() {
                return Ok(());
            }
            let now = Instant::now();
            if self.timer.ready(now) || self.starved() {
                match self.announce(self.announce_event()) {
                    Ok(response) => {
                        on_event(&Event::Announced {
                            peers: response.peers.len(),
                        });
                        self.timer.after_success(response.interval, Instant::now());
                        for addr in response.peers {
                            if self.known.insert(addr) {
                                self.queue.push(addr);
                            }
                        }
                        if self.starved() {
                            self.empty_rounds += 1;
                            if self.empty_rounds >= MAX_EMPTY_ROUNDS {
                                return Err(Error::NoPeers);
                            }
                            std::thread::sleep(Duration::from_secs(1));
                            self.timer = AnnounceTimer::new();
                            continue;
                        }
                        self.empty_rounds = 0;
                    }
                    Err(e) => {
                        self.timer.after_failure(Instant::now());
                        if self.starved() {
                            self.empty_rounds += 1;
                            if self.empty_rounds >= MAX_EMPTY_ROUNDS {
                                return Err(e);
                            }
                            std::thread::sleep(Duration::from_secs(1));
                            self.timer = AnnounceTimer::new();
                            continue;
                        }
                    }
                }
            }
            while self.connecting + self.peers.len() < self.config.max_peers
                && !self.queue.is_empty()
            {
                let addr = self.queue.remove(0);
                self.spawn_connector(addr, next_id);
                next_id += 1;
                self.connecting += 1;
            }
            let count_before = self.peers.len();
            match rx_in.recv_timeout(Duration::from_millis(500)) {
                Ok(input) => self.handle(input, on_event)?,
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return Err(Error::Incomplete),
            }
            self.housekeeping(on_event)?;
            if self.peers.len() != count_before {
                on_event(&Event::Peers {
                    count: self.peers.len(),
                });
            }
        }
    }

    fn starved(&self) -> bool {
        self.peers.is_empty() && self.connecting == 0 && self.queue.is_empty()
    }

    fn announce_event(&self) -> TrackerEvent {
        if self.downloaded == 0 && self.known.is_empty() {
            TrackerEvent::Started
        } else {
            TrackerEvent::None
        }
    }

    fn announce(
        &mut self,
        event: TrackerEvent,
    ) -> Result<crate::tracker::AnnounceResponse, Error> {
        let request = AnnounceRequest {
            info_hash: self.meta.info_hash,
            peer_id: self.config.peer_id,
            port: self.config.port,
            uploaded: self.uploaded,
            downloaded: self.downloaded,
            left: self.meta.total_length.saturating_sub(self.downloaded),
            event,
        };
        self.trackers
            .announce(&request, &mut |url, req| {
                announce_url(url, req, ANNOUNCE_TIMEOUT)
            })
            .map_err(|e| Error::Io(format!("{:?}", e)))
    }

    fn spawn_connector(&self, addr: SocketAddr, id: u64) {
        let tx_in = self.tx_in.clone();
        let mine = Handshake {
            info_hash: self.meta.info_hash,
            peer_id: self.config.peer_id,
            extensions: false,
        };
        std::thread::spawn(move || {
            let connected = (|| {
                let mut stream = TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT).ok()?;
                stream.set_read_timeout(Some(READ_TIMEOUT)).ok()?;
                stream.set_write_timeout(Some(READ_TIMEOUT)).ok()?;
                exchange_handshake(&mut stream, &mine).ok()?;
                Some(stream)
            })();
            let stream = match connected {
                Some(s) => s,
                None => {
                    let _ = tx_in.send(In::ConnectFailed);
                    return;
                }
            };
            let (tx_out, rx_out) = channel::<Message>();
            let mut write_half = match stream.try_clone() {
                Ok(s) => s,
                Err(_) => {
                    let _ = tx_in.send(In::ConnectFailed);
                    return;
                }
            };
            let engine_stream = match stream.try_clone() {
                Ok(s) => s,
                Err(_) => {
                    let _ = tx_in.send(In::ConnectFailed);
                    return;
                }
            };
            std::thread::spawn(move || loop {
                match rx_out.recv_timeout(Duration::from_secs(60)) {
                    Ok(message) => {
                        if write_message(&mut write_half, &message).is_err() {
                            return;
                        }
                    }
                    Err(RecvTimeoutError::Timeout) => {
                        if write_message(&mut write_half, &Message::KeepAlive).is_err() {
                            return;
                        }
                    }
                    Err(RecvTimeoutError::Disconnected) => {
                        let _ = write_half.shutdown(Shutdown::Both);
                        return;
                    }
                }
            });
            if tx_in
                .send(In::Connected {
                    id,
                    addr,
                    tx: tx_out,
                    stream: engine_stream,
                })
                .is_err()
            {
                return;
            }
            let mut read_half = stream;
            loop {
                match read_message(&mut read_half) {
                    Ok(message) => {
                        if tx_in.send(In::Msg { id, message }).is_err() {
                            return;
                        }
                    }
                    Err(_) => {
                        let _ = tx_in.send(In::Died { id });
                        return;
                    }
                }
            }
        });
    }

    fn handle(&mut self, input: In, on_event: &mut dyn FnMut(&Event)) -> Result<(), Error> {
        match input {
            In::Connected {
                id,
                addr,
                tx,
                stream,
            } => {
                self.connecting = self.connecting.saturating_sub(1);
                let total = self.meta.pieces.len() as u32;
                let peer = Peer {
                    tx,
                    stream,
                    state: PeerState::new(),
                    theirs: Bitfield::new(total),
                    theirs_known: false,
                    job: None,
                    pipeline: RequestPipeline::new(PIPELINE_DEPTH, BLOCK_TIMEOUT),
                    downloaded_from: 0,
                    last_useful: Instant::now(),
                };
                if self.ours.count_set() > 0 {
                    let _ = peer.tx.send(Message::Bitfield(self.ours.as_bytes().to_vec()));
                }
                self.peers.insert(id, peer);
                on_event(&Event::Connected { addr });
            }
            In::ConnectFailed => {
                self.connecting = self.connecting.saturating_sub(1);
            }
            In::Died { id } => {
                self.remove_peer(id);
            }
            In::Msg { id, message } => {
                self.on_message(id, message, on_event)?;
            }
        }
        Ok(())
    }

    fn remove_peer(&mut self, id: u64) {
        if let Some(peer) = self.peers.remove(&id) {
            if peer.theirs_known {
                self.availability.remove_bitfield(&peer.theirs);
            }
            if let Some(job) = &peer.job {
                self.picker.abandon(job.index);
            }
            let _ = peer.stream.shutdown(Shutdown::Both);
        }
    }

    fn on_message(
        &mut self,
        id: u64,
        message: Message,
        on_event: &mut dyn FnMut(&Event),
    ) -> Result<(), Error> {
        let total = self.meta.pieces.len() as u32;
        let peer = match self.peers.get_mut(&id) {
            Some(p) => p,
            None => return Ok(()),
        };
        peer.state.on_message(&message);
        match message {
            Message::Bitfield(bytes) => match Bitfield::from_bytes(&bytes, total) {
                Ok(bits) => {
                    if peer.theirs_known {
                        self.availability.remove_bitfield(&peer.theirs);
                    }
                    peer.theirs = bits;
                    peer.theirs_known = true;
                    self.availability.add_bitfield(&peer.theirs);
                }
                Err(_) => {
                    self.remove_peer(id);
                    return Ok(());
                }
            },
            Message::Have(index) => {
                if !peer.theirs.has(index) {
                    peer.theirs.set(index);
                    peer.theirs_known = true;
                    self.availability.add_have(index);
                }
            }
            Message::Choke => {
                if let Some(job) = peer.job.as_mut() {
                    for block in peer.pipeline.drain() {
                        job.pending.push((block.begin, block.length));
                    }
                }
            }
            Message::Request {
                index,
                begin,
                length,
            } => {
                if peer.state.may_upload()
                    && self.ours.has(index)
                    && length <= MAX_SERVED_BLOCK
                    && length > 0
                {
                    if let Ok(data) = self.storage.read_block(index, begin, length as u64) {
                        if peer.tx.send(Message::Piece { index, begin, data }).is_ok() {
                            self.uploaded += length as u64;
                        }
                    }
                }
            }
            Message::Piece { index, begin, data } => {
                peer.downloaded_from += data.len() as u64;
                peer.last_useful = Instant::now();
                let mut finished: Option<PieceJob> = None;
                if let Some(job) = peer.job.as_mut() {
                    if job.index == index
                        && peer
                            .pipeline
                            .complete(index, begin, data.len() as u32)
                            .is_some()
                    {
                        let start = begin as usize;
                        if start + data.len() <= job.data.len() {
                            job.data[start..start + data.len()].copy_from_slice(&data);
                            job.received += data.len() as u64;
                        }
                        if job.done() {
                            finished = peer.job.take();
                        }
                    }
                }
                if let Some(job) = finished {
                    self.finish_piece(id, job, on_event)?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn finish_piece(
        &mut self,
        id: u64,
        mut job: PieceJob,
        on_event: &mut dyn FnMut(&Event),
    ) -> Result<(), Error> {
        let index = job.index;
        if verify_piece(&job.data, &self.meta.pieces[index as usize]) {
            self.storage
                .write_block(index, 0, &job.data)
                .map_err(|e| Error::Io(e.to_string()))?;
            self.ours.set(index);
            self.downloaded += job.data.len() as u64;
            self.picker.complete(index);
            for (other_id, other) in self.peers.iter_mut() {
                let _ = other.tx.send(Message::Have(index));
                if *other_id != id {
                    if other.job.as_ref().map(|j| j.index) == Some(index) {
                        for block in other.pipeline.drain() {
                            let _ = other.tx.send(Message::Cancel {
                                index: block.piece,
                                begin: block.begin,
                                length: block.length,
                            });
                        }
                        other.job = None;
                    }
                }
            }
            on_event(&Event::PieceDone {
                index,
                have: self.ours.count_set(),
                total: self.meta.pieces.len() as u32,
            });
        } else if job.failures + 1 >= MAX_VERIFY_FAILURES {
            self.picker.abandon(index);
            self.remove_peer(id);
        } else {
            job.restart();
            if let Some(peer) = self.peers.get_mut(&id) {
                peer.job = Some(job);
            } else {
                self.picker.abandon(index);
            }
        }
        Ok(())
    }

    fn housekeeping(&mut self, _on_event: &mut dyn FnMut(&Event)) -> Result<(), Error> {
        let now = Instant::now();
        let mut dead: Vec<u64> = Vec::new();
        let mut assigned: Vec<u64> = Vec::new();
        let ids: Vec<u64> = self.peers.keys().copied().collect();
        for id in ids {
            let peer = self.peers.get_mut(&id).unwrap();
            if let Some(job) = peer.job.as_mut() {
                for block in peer.pipeline.expired(now) {
                    job.pending.push((block.begin, block.length));
                }
            }
            if peer.job.is_none() && peer.theirs_known {
                if let Some(index) =
                    self.picker.pick(&self.ours, &peer.theirs, &self.availability)
                {
                    peer.job = Some(PieceJob::start(index, self.storage.piece_size(index)));
                    assigned.push(id);
                }
            }
            if peer.job.is_some()
                && now.duration_since(peer.last_useful) > SNUB_TIMEOUT
                && !self.ours.is_complete()
            {
                dead.push(id);
                continue;
            }
            if let Some(job) = peer.job.as_mut() {
                if !peer.state.am_interested {
                    if peer.tx.send(Message::Interested).is_err() {
                        dead.push(id);
                        continue;
                    }
                    peer.state.set_interested(true);
                }
                if peer.state.can_request() {
                    while peer.pipeline.has_slot() && !job.pending.is_empty() {
                        let (begin, length) = job.pending.remove(0);
                        let block = BlockRequest {
                            piece: job.index,
                            begin,
                            length,
                        };
                        if peer.pipeline.issue(block, now)
                            && peer
                                .tx
                                .send(Message::Request {
                                    index: job.index,
                                    begin,
                                    length,
                                })
                                .is_err()
                        {
                            dead.push(id);
                            break;
                        }
                    }
                }
            }
        }
        let _ = assigned;
        for id in dead {
            self.remove_peer(id);
        }
        if now.duration_since(self.last_choke) >= CHOKE_INTERVAL {
            self.last_choke = now;
            self.rebalance_chokes(now);
        }
        Ok(())
    }

    fn final_check(&mut self) -> u32 {
        let mut bad = 0u32;
        for index in 0..self.meta.pieces.len() as u32 {
            let ok = match self.storage.read_piece(index) {
                Ok(data) => verify_piece(&data, &self.meta.pieces[index as usize]),
                Err(_) => false,
            };
            if !ok {
                bad += 1;
                self.ours.clear(index);
                self.downloaded = self
                    .downloaded
                    .saturating_sub(self.storage.piece_size(index));
            }
        }
        bad
    }

    fn rebalance_chokes(&mut self, now: Instant) {
        let candidates: Vec<u64> = self
            .peers
            .iter()
            .filter(|(_, p)| p.state.peer_interested && p.state.am_choking)
            .map(|(id, _)| *id)
            .collect();
        let optimistic = self.rotor.maybe_rotate(&candidates, now);
        let rates: Vec<PeerRate> = self
            .peers
            .iter()
            .map(|(id, p)| PeerRate {
                id: *id,
                downloaded_from: p.downloaded_from,
                interested: p.state.peer_interested,
            })
            .collect();
        let unchoked = select_unchoked(&rates, REGULAR_UNCHOKE_SLOTS, optimistic);
        for (id, peer) in self.peers.iter_mut() {
            let should_unchoke = unchoked.contains(id);
            if should_unchoke && peer.state.am_choking {
                if peer.tx.send(Message::Unchoke).is_ok() {
                    peer.state.set_choking(false);
                }
            } else if !should_unchoke && !peer.state.am_choking {
                if peer.tx.send(Message::Choke).is_ok() {
                    peer.state.set_choking(true);
                }
            }
        }
    }
}
