//! Peer-to-peer Automerge sync over TCP.
//!
//! Wire format per frame: `[u8 doc_kind][u32 BE length][payload]`, payload is an
//! encoded `automerge::sync::Message`. Frames over [`MAX_FRAME`] close the connection.
//! This crate sees docs only as `Automerge` values, never domain types.

pub mod config;
pub mod persist;

use automerge::{
    sync::{Message, State, SyncDoc},
    Automerge,
};
use std::{
    collections::{BTreeMap, HashSet},
    io,
    net::IpAddr,
    sync::{Arc, Mutex, MutexGuard},
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::{mpsc, watch},
    task::JoinSet,
};

pub const DEFAULT_PORT: u16 = 7420;
pub const MAX_FRAME: u32 = 16 * 1024 * 1024;
pub const POLL: Duration = Duration::from_secs(30);
const MAX_BACKOFF: Duration = Duration::from_secs(30);

/// Doc kinds on the wire.
pub const TODO: u8 = 1;
pub const FITNESS: u8 = 2;

/// A doc shared between the app and the sync loop. Stores implement `AsMut<Automerge>`.
pub type Doc = Arc<Mutex<dyn AsMut<Automerge> + Send>>;

fn invalid(msg: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.into())
}

fn lock(doc: &Doc) -> io::Result<MutexGuard<'_, dyn AsMut<Automerge> + Send + 'static>> {
    doc.lock().map_err(|_| io::Error::other("doc mutex poisoned"))
}

pub async fn read_frame<R: AsyncRead + Unpin>(r: &mut R) -> io::Result<(u8, Vec<u8>)> {
    let mut header = [0u8; 5];
    r.read_exact(&mut header).await?;
    let len = u32::from_be_bytes([header[1], header[2], header[3], header[4]]);
    if len > MAX_FRAME {
        // Check before allocating: a hostile length can't make us reserve memory.
        return Err(invalid(format!("frame of {len} bytes exceeds limit")));
    }
    let mut payload = vec![0u8; len as usize];
    r.read_exact(&mut payload).await?;
    Ok((header[0], payload))
}

pub async fn write_frame<W: AsyncWrite + Unpin>(w: &mut W, kind: u8, payload: &[u8]) -> io::Result<()> {
    let len = u32::try_from(payload.len()).ok().filter(|l| *l <= MAX_FRAME);
    let len = len.ok_or_else(|| invalid("outgoing sync message exceeds frame limit"))?;
    let mut buf = Vec::with_capacity(5 + payload.len());
    buf.push(kind);
    buf.extend_from_slice(&len.to_be_bytes());
    buf.extend_from_slice(payload);
    w.write_all(&buf).await?;
    w.flush().await
}

/// Bind the sync listener. Only Tailscale (100.64.0.0/10) or loopback addresses are
/// accepted; never the unspecified address (0.0.0.0 / ::).
pub async fn bind(ip: IpAddr, port: u16) -> io::Result<TcpListener> {
    let ok = match ip {
        IpAddr::V4(v4) => v4.is_loopback() || (v4.octets()[0] == 100 && (v4.octets()[1] & 0xC0) == 64),
        IpAddr::V6(v6) => v6.is_loopback(),
    };
    if !ok {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("refusing to listen on {ip}: use the device's Tailscale IPv4 (100.64.0.0/10) or 127.0.0.1"),
        ));
    }
    TcpListener::bind((ip, port)).await
}

/// `"host"` → `"host:7420"`; `"host:port"` unchanged.
fn with_port(peer: &str) -> String {
    if peer.rsplit_once(':').is_some_and(|(_, p)| p.parse::<u16>().is_ok()) {
        peer.to_string()
    } else {
        format!("{peer}:{DEFAULT_PORT}")
    }
}

struct Inner {
    docs: BTreeMap<u8, Doc>,
    peers: Vec<String>,
    poll: Duration,
    /// Bumped on every local or remote change; wakes all connections.
    wake: watch::Sender<u64>,
    /// Doc kinds changed by a remote peer (the app persists them / refreshes UI).
    remote_changed: mpsc::UnboundedSender<u8>,
}

#[derive(Clone)]
pub struct SyncNode(Arc<Inner>);

impl SyncNode {
    /// `peers` are MagicDNS names or IPs, optionally `:port`. They are both the
    /// dial list and the allowlist for incoming connections.
    pub fn new(docs: BTreeMap<u8, Doc>, peers: Vec<String>, poll: Duration) -> (Self, mpsc::UnboundedReceiver<u8>) {
        let (remote_changed, rx) = mpsc::unbounded_channel();
        let (wake, _) = watch::channel(0);
        (SyncNode(Arc::new(Inner { docs, peers, poll, wake, remote_changed })), rx)
    }

    /// Call after every local edit so connected peers get it right away.
    pub fn local_change(&self) {
        self.0.wake.send_modify(|n| *n += 1);
    }

    /// Accept from allowlisted peers and dial every peer with reconnect backoff.
    /// Runs until the returned future is dropped (aborting it stops all tasks).
    pub async fn run(self, listener: TcpListener) -> io::Result<()> {
        let mut tasks = JoinSet::new();
        for peer in self.0.peers.clone() {
            tasks.spawn(self.clone().dial_loop(peer));
        }
        loop {
            while tasks.try_join_next().is_some() {}
            let (stream, addr) = listener.accept().await?;
            if !self.allowed(addr.ip()).await {
                eprintln!("sync: dropping connection from unlisted {addr}");
                continue; // `stream` dropped here = connection closed
            }
            let node = self.clone();
            tasks.spawn(async move {
                if let Err(e) = node.connection(stream).await {
                    eprintln!("sync: {addr} (incoming): {e}");
                }
            });
        }
    }

    /// Resolved on every accept so MagicDNS changes are picked up.
    async fn allowed(&self, ip: IpAddr) -> bool {
        let mut ips = HashSet::new();
        for p in &self.0.peers {
            if let Ok(addrs) = tokio::net::lookup_host(with_port(p)).await {
                ips.extend(addrs.map(|a| a.ip()));
            }
        }
        ips.contains(&ip.to_canonical())
    }

    async fn dial_loop(self, peer: String) {
        let target = with_port(&peer);
        let mut backoff = Duration::from_secs(1);
        loop {
            match TcpStream::connect(&target).await {
                Ok(stream) => {
                    backoff = Duration::from_secs(1);
                    if let Err(e) = self.connection(stream).await {
                        eprintln!("sync: {target} (outgoing): {e}");
                    }
                }
                Err(e) => eprintln!("sync: connect {target}: {e}; retry in {backoff:?}"),
            }
            tokio::time::sleep(backoff).await;
            backoff = (backoff * 2).min(MAX_BACKOFF);
        }
    }

    /// One connection: a fresh `sync::State` per doc. Loop: send whatever each
    /// doc's state says the peer is missing, then wait for a frame, a local
    /// change, or the poll tick. Returns on any error or EOF.
    async fn connection(&self, stream: TcpStream) -> io::Result<()> {
        let (mut rd, mut wr) = stream.into_split();
        // Reading in its own task keeps `read_frame` out of `select!` (it isn't cancel-safe).
        let (tx, mut frames) = mpsc::channel(8);
        let reader = tokio::spawn(async move {
            loop {
                let f = read_frame(&mut rd).await;
                let stop = f.is_err();
                if tx.send(f).await.is_err() || stop {
                    break;
                }
            }
        });
        let _abort_reader = AbortOnDrop(reader);

        let mut states: BTreeMap<u8, State> = self.0.docs.keys().map(|k| (*k, State::new())).collect();
        let mut wake = self.0.wake.subscribe();
        let mut tick = tokio::time::interval(self.0.poll);
        loop {
            for (kind, doc) in &self.0.docs {
                let state = states.get_mut(kind).expect("states built from docs");
                let msg = lock(doc)?.as_mut().generate_sync_message(state);
                if let Some(msg) = msg {
                    write_frame(&mut wr, *kind, &msg.encode()).await?;
                }
            }
            tokio::select! {
                f = frames.recv() => {
                    let (kind, payload) = f.ok_or_else(|| io::Error::from(io::ErrorKind::UnexpectedEof))??;
                    self.receive(&mut states, kind, &payload)?;
                }
                r = wake.changed() => r.map_err(io::Error::other)?,
                _ = tick.tick() => {}
            }
        }
    }

    fn receive(&self, states: &mut BTreeMap<u8, State>, kind: u8, payload: &[u8]) -> io::Result<()> {
        let (doc, state) = match (self.0.docs.get(&kind), states.get_mut(&kind)) {
            (Some(d), Some(s)) => (d, s),
            _ => return Err(invalid(format!("unknown doc kind {kind}"))),
        };
        let msg = Message::decode(payload).map_err(|e| invalid(format!("bad sync message: {e}")))?;
        let changed = {
            let mut guard = lock(doc)?;
            let d = guard.as_mut();
            let before = d.get_heads();
            d.receive_sync_message(state, msg).map_err(|e| invalid(format!("rejected sync message: {e}")))?;
            d.get_heads() != before
        };
        if changed {
            let _ = self.0.remote_changed.send(kind);
            self.local_change(); // forward to other connected peers
        }
        Ok(())
    }
}

struct AbortOnDrop(tokio::task::JoinHandle<()>);
impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

#[cfg(test)]
mod tests;
