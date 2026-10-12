//! Change notifications. `Events` is a hub the server (and `peer`, after a peer merge or an
//! edit made on disk) tells whenever a doc changed; `GET /api/v1/events` streams them to
//! browsers as Server-Sent Events so an open screen can refetch.
//!
//! Each message is `{"doc": "todo" | "fitness" | "shopping", "version": <n>}`; `version` is a
//! per-doc change counter for this process (not an Automerge hash), only useful as "it moved".
//! A client that connects (or reconnects after a drop, which `EventSource` does on its own)
//! first gets one message per doc with the current counter, so it can refetch whatever it
//! missed while disconnected.

use crate::{identity::Me, AppState, DocKind};
use axum::{
    extract::State,
    response::sse::{Event, KeepAlive, Sse},
};
use futures_util::stream::{self, Stream};
use std::{
    collections::VecDeque,
    convert::Infallible,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::sync::broadcast::{self, error::RecvError};

const KEEP_ALIVE: Duration = Duration::from_secs(15);

#[derive(Clone)]
pub struct Events(Arc<Inner>);

struct Inner {
    tx: broadcast::Sender<(DocKind, u64)>,
    versions: [AtomicU64; 3],
}

impl Events {
    pub fn new() -> Self {
        Events(Arc::new(Inner { tx: broadcast::channel(64).0, versions: Default::default() }))
    }

    /// A doc changed (a local write, a merge from a peer, or an edit found on disk).
    pub fn notify(&self, kind: DocKind) {
        let v = self.0.versions[kind.index()].fetch_add(1, Ordering::SeqCst) + 1;
        let _ = self.0.tx.send((kind, v)); // no subscribers is fine
    }

    pub fn version(&self, kind: DocKind) -> u64 {
        self.0.versions[kind.index()].load(Ordering::SeqCst)
    }

    fn subscribe(&self) -> broadcast::Receiver<(DocKind, u64)> {
        self.0.tx.subscribe()
    }
}

impl Default for Events {
    fn default() -> Self {
        Self::new()
    }
}

fn message(kind: DocKind, version: u64) -> Event {
    Event::default().json_data(serde_json::json!({ "doc": kind.name(), "version": version })).expect("a small JSON object serializes")
}

/// `GET /events`. Subscribe before reading the counters, so a change in between is not lost.
pub(crate) async fn stream(State(st): State<AppState>, _me: Me) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let events = st.0.events.clone();
    let rx = events.subscribe();
    let queue: VecDeque<(DocKind, u64)> = DocKind::ALL.iter().map(|k| (*k, events.version(*k))).collect();
    let s = stream::unfold((rx, queue, events), |(mut rx, mut queue, events)| async move {
        loop {
            if let Some((kind, version)) = queue.pop_front() {
                return Some((Ok(message(kind, version)), (rx, queue, events)));
            }
            match rx.recv().await {
                Ok(msg) => queue.push_back(msg),
                // Fell behind: say "everything moved" with the current counters.
                Err(RecvError::Lagged(_)) => queue.extend(DocKind::ALL.iter().map(|k| (*k, events.version(*k)))),
                Err(RecvError::Closed) => return None,
            }
        }
    });
    // A comment line every 15 s keeps proxies (tailscale serve) from closing an idle stream.
    Sse::new(s).keep_alive(KeepAlive::new().interval(KEEP_ALIVE).text("keep-alive"))
}
