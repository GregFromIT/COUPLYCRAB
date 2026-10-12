//! Localhost-only network tests.

use super::*;
use fitness_core::{AmFitnessStore, FitnessStore, MeasurementType};
use std::net::Ipv4Addr;
use todo_core::{AmTodoStore, TodoStore};
use tokio::time::{sleep, timeout, Instant};

const LOCAL: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);

struct Peer {
    todo: Arc<Mutex<AmTodoStore>>,
    fit: Arc<Mutex<AmFitnessStore>>,
    node: SyncNode,
    task: tokio::task::JoinHandle<io::Result<()>>,
}

fn stores() -> (Arc<Mutex<AmTodoStore>>, Arc<Mutex<AmFitnessStore>>) {
    (Arc::new(Mutex::new(AmTodoStore::new())), Arc::new(Mutex::new(AmFitnessStore::new())))
}

fn start(listener: TcpListener, peers: Vec<String>, todo: Arc<Mutex<AmTodoStore>>, fit: Arc<Mutex<AmFitnessStore>>) -> Peer {
    let docs: BTreeMap<u8, Doc> = BTreeMap::from([(TODO, todo.clone() as Doc), (FITNESS, fit.clone() as Doc)]);
    let (node, _rx) = SyncNode::new(docs, peers, Duration::from_millis(200));
    let task = tokio::spawn(node.clone().run(listener));
    Peer { todo, fit, node, task }
}

async fn listener() -> (TcpListener, String) {
    let l = bind(LOCAL, 0).await.unwrap();
    let addr = l.local_addr().unwrap().to_string();
    (l, addr)
}

fn heads(d: &mut dyn AsMut<Automerge>) -> Vec<automerge::ChangeHash> {
    let mut h = d.as_mut().get_heads();
    h.sort();
    h
}

async fn converged(a: &Peer, b: &Peer) -> bool {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        let same = heads(&mut *a.todo.lock().unwrap()) == heads(&mut *b.todo.lock().unwrap())
            && heads(&mut *a.fit.lock().unwrap()) == heads(&mut *b.fit.lock().unwrap());
        if same {
            return true;
        }
        sleep(Duration::from_millis(50)).await;
    }
    false
}

#[tokio::test]
async fn two_peers_converge_both_doc_kinds() {
    let ((la, aa), (lb, ab)) = (listener().await, listener().await);
    let (ta, fa) = stores();
    let (tb, fb) = stores();
    ta.lock().unwrap().add("from a", "", None).unwrap();
    fb.lock().unwrap().add_exercise("Squat", MeasurementType::WeightReps, None).unwrap();
    let a = start(la, vec![ab], ta, fa);
    let b = start(lb, vec![aa], tb, fb);
    assert!(converged(&a, &b).await);
    assert_eq!(a.todo.lock().unwrap().list().unwrap(), b.todo.lock().unwrap().list().unwrap());
    assert_eq!(a.fit.lock().unwrap().list_exercises().unwrap()[0].name, "Squat");

    // A change after connecting arrives via the local-change trigger.
    b.todo.lock().unwrap().add("later", "", None).unwrap();
    b.node.local_change();
    assert!(converged(&a, &b).await);
    assert_eq!(a.todo.lock().unwrap().list().unwrap().len(), 2);
}

#[tokio::test]
async fn offline_change_arrives_after_reconnect() {
    let (la, aa) = listener().await;
    let (ta, fa) = stores();
    // A only accepts (its "dial" target is a closed port); B dials A.
    let a = start(la, vec!["127.0.0.1:1".into()], ta, fa);
    let (tb, fb) = stores();
    let (lb, _) = listener().await;
    let b = start(lb, vec![aa.clone()], tb.clone(), fb.clone());
    assert!(converged(&a, &b).await);

    // B goes offline; A changes; B comes back with the same docs.
    b.task.abort();
    let _ = b.task.await;
    a.todo.lock().unwrap().add("while offline", "", None).unwrap();
    a.node.local_change();
    let (lb, _) = listener().await;
    let b = start(lb, vec![aa], tb, fb);
    assert!(converged(&a, &b).await);
    assert_eq!(b.todo.lock().unwrap().list().unwrap()[0].title, "while offline");
}

/// Connect a raw client, send `bytes`, and return true if the server closes the
/// connection (EOF or reset) within 5 s. Server-sent sync frames are discarded.
async fn server_closes_after(addr: &str, bytes: &[u8]) -> bool {
    let mut s = TcpStream::connect(addr).await.unwrap();
    let _ = s.write_all(bytes).await;
    let mut sink = Vec::new();
    timeout(Duration::from_secs(5), s.read_to_end(&mut sink)).await.is_ok()
}

#[tokio::test]
async fn oversize_frame_closes_connection() {
    let (la, aa) = listener().await;
    let (ta, fa) = stores();
    let a = start(la, vec!["127.0.0.1:1".into()], ta, fa);
    let mut header = vec![TODO];
    header.extend_from_slice(&(MAX_FRAME + 1).to_be_bytes());
    assert!(server_closes_after(&aa, &header).await);
    // Still serving afterwards.
    let (tb, fb) = stores();
    let (lb, _) = listener().await;
    let b = start(lb, vec![aa], tb, fb);
    assert!(converged(&a, &b).await);
    assert!(!a.task.is_finished());
}

#[tokio::test]
async fn malformed_payload_rejected_without_corruption() {
    let (la, aa) = listener().await;
    let (ta, fa) = stores();
    ta.lock().unwrap().add("keep me", "", None).unwrap();
    let a = start(la, vec!["127.0.0.1:1".into()], ta, fa);
    let before = heads(&mut *a.todo.lock().unwrap());

    // Garbage payload, unknown doc kind, and a real message with its tail corrupted.
    let mut good = AmTodoStore::new();
    good.add("x", "", None).unwrap();
    let mut corrupt = good.doc.generate_sync_message(&mut State::new()).unwrap().encode();
    let n = corrupt.len();
    corrupt[n / 2..].iter_mut().for_each(|b| *b = 0xFF);
    for (kind, payload) in [(TODO, b"garbage".to_vec()), (99, b"x".to_vec()), (TODO, corrupt)] {
        let mut frame = vec![kind];
        frame.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        frame.extend_from_slice(&payload);
        assert!(server_closes_after(&aa, &frame).await);
    }
    let mut t = a.todo.lock().unwrap();
    assert_eq!(heads(&mut *t), before);
    let reloaded = Automerge::load(&t.doc.save()).unwrap();
    assert_eq!(AmTodoStore::from_doc(reloaded).unwrap().list().unwrap()[0].title, "keep me");
    assert!(!a.task.is_finished());
}

#[tokio::test]
async fn unlisted_ip_is_dropped() {
    let (la, aa) = listener().await;
    let (ta, fa) = stores();
    let _a = start(la, vec!["127.0.0.2:1".into()], ta, fa);
    // An allowed peer would immediately receive A's first sync frame; we get nothing.
    let mut s = TcpStream::connect(&aa).await.unwrap();
    let mut buf = Vec::new();
    let r = timeout(Duration::from_secs(5), s.read_to_end(&mut buf)).await;
    assert!(r.is_ok(), "connection should be closed");
    assert!(buf.is_empty(), "no data should be sent to an unlisted peer");
}

#[tokio::test]
async fn bind_refuses_unspecified_and_public() {
    for ip in ["0.0.0.0", "::", "192.168.1.5", "8.8.8.8"] {
        assert!(bind(ip.parse().unwrap(), 0).await.is_err(), "{ip}");
    }
}

#[tokio::test]
async fn frame_roundtrip_and_limit() {
    let mut buf = Vec::new();
    write_frame(&mut buf, FITNESS, b"hello").await.unwrap();
    assert_eq!(read_frame(&mut buf.as_slice()).await.unwrap(), (FITNESS, b"hello".to_vec()));
    let big = vec![0u8; MAX_FRAME as usize + 1];
    assert!(write_frame(&mut Vec::new(), TODO, &big).await.is_err());
}
