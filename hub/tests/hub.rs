//! The M1 test list of `DESIGN.md` section 7, in order.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::Duration;

use message_hub::{
    Callback, DataId, Event, Hub, Publisher, Scheduler, Subscription, Value, ValueError, ValueKind,
};

/// One recorded delivery: `(id, Some((kind, bytes)))`, or `(id, None)` for `Closed`.
type Item = (DataId, Option<(ValueKind, Vec<u8>)>);
type Receiver = mpsc::Receiver<Item>;

fn item(id: DataId, ev: Event<'_>) -> Item {
    match ev {
        Event::Value(kind, bytes) => (id, Some((kind, bytes.to_vec()))),
        Event::Closed => (id, None),
    }
}

fn value(id: DataId, kind: ValueKind, bytes: &[u8]) -> Item {
    (id, Some((kind, bytes.to_vec())))
}

/// A callback that records every delivery into the returned receiver.
fn sink() -> (Callback, Receiver) {
    let (tx, rx) = mpsc::channel();
    let tx = Mutex::new(tx);
    let cb: Callback = Arc::new(move |id, ev| tx.lock().unwrap().send(item(id, ev)).unwrap());
    (cb, rx)
}

/// Blocks for exactly `n` deliveries; a missing one fails the test instead of hanging.
fn take<T>(rx: &mpsc::Receiver<T>, n: usize) -> Vec<T> {
    (0..n)
        .map(|_| rx.recv_timeout(Duration::from_secs(5)).expect("delivery"))
        .collect()
}

fn drain(rx: &Receiver) -> Vec<Item> {
    let mut out = Vec::new();
    while let Ok(x) = rx.try_recv() {
        out.push(x);
    }
    out
}

#[test]
fn subscribe_publish_delivers_id_kind_and_bytes() {
    let hub = Hub::new();
    let p = hub.publisher(7, ValueKind::String);
    let (cb, rx) = sink();
    let _s = hub.subscribe(7, cb);
    p.publish(b"hi");
    assert_eq!(take(&rx, 1), [value(7, ValueKind::String, b"hi")]);
}

#[test]
fn publish_without_subscribers_is_silent_and_pull_works() {
    let hub = Hub::new();
    let p = hub.publisher(1, ValueKind::U32);
    p.publish(&1u32.to_le_bytes());
    assert_eq!(hub.get(1).unwrap().1.as_ref(), 1u32.to_le_bytes());
}

#[test]
fn subscribers_receive_in_registration_order() {
    let hub = Hub::new();
    let p = hub.publisher(1, ValueKind::U8);
    let order = Arc::new(Mutex::new(Vec::new()));
    let subs: Vec<Subscription> = (0..3)
        .map(|i| {
            let order = Arc::clone(&order);
            hub.subscribe(1, Arc::new(move |_, _| order.lock().unwrap().push(i)))
        })
        .collect();
    p.publish(&[9]);
    assert_eq!(*order.lock().unwrap(), [0, 1, 2]);
    drop(subs);
}

#[test]
fn dropping_subscription_stops_deliveries() {
    let hub = Hub::new();
    let p = hub.publisher(1, ValueKind::U8);
    let (cb, rx) = sink();
    let s = hub.subscribe(1, cb);
    p.publish(&[1]);
    drop(s);
    p.publish(&[2]);
    assert_eq!(take(&rx, 1), [value(1, ValueKind::U8, &[1])]);
    assert!(drain(&rx).is_empty());
}

#[test]
fn unsubscribe_is_idempotent() {
    let hub = Hub::new();
    let p = hub.publisher(1, ValueKind::U8);
    let (cb, rx) = sink();
    let s = hub.subscribe(1, cb);
    s.unsubscribe();
    s.unsubscribe();
    p.publish(&[1]);
    drop(s);
    assert!(drain(&rx).is_empty());
}

#[test]
fn get_returns_the_latest_after_several_publishes() {
    let hub = Hub::new();
    let p = hub.publisher(1, ValueKind::U32);
    for v in [1u32, 2, 3] {
        p.publish(&v.to_le_bytes());
    }
    assert_eq!(hub.get(1).unwrap().1.as_ref(), 3u32.to_le_bytes());
}

#[test]
fn subscribing_does_not_replay_the_last_value() {
    let hub = Hub::new();
    let p = hub.publisher(1, ValueKind::U8);
    p.publish(&[1]);
    let (cb, rx) = sink();
    let _s = hub.subscribe(1, cb);
    assert!(drain(&rx).is_empty());
    // pull is the way to the current value
    assert_eq!(hub.get(1).unwrap().1.as_ref(), [1]);
}

#[test]
fn unsubscribing_inside_a_callback_completes_it_and_skips_earlier_unsubscribed() {
    let hub = Hub::new();
    let p = hub.publisher(1, ValueKind::U8);

    // a subscriber dropping its own subscription: this delivery completes, the next one misses
    let own: Arc<Mutex<Option<Subscription>>> = Arc::new(Mutex::new(None));
    let (tx, rx_self) = mpsc::channel();
    let tx = Mutex::new(tx);
    let own_in_cb = Arc::clone(&own);
    let s = hub.subscribe(
        1,
        Arc::new(move |id, ev| {
            tx.lock().unwrap().send(item(id, ev)).unwrap();
            let taken = own_in_cb.lock().unwrap().take();
            drop(taken);
        }),
    );
    *own.lock().unwrap() = Some(s);

    // A, registered first, unsubscribes B during the first dispatch: B is skipped although it
    // is in that snapshot, and gone from the next one
    let b_slot: Arc<Mutex<Option<Subscription>>> = Arc::new(Mutex::new(None));
    let a_hits = Arc::new(AtomicUsize::new(0));
    let (cb_b, rx_b) = sink();
    let (slot_in_a, hits_in_a) = (Arc::clone(&b_slot), Arc::clone(&a_hits));
    let _a = hub.subscribe(
        1,
        Arc::new(move |_, _| {
            hits_in_a.fetch_add(1, Ordering::Relaxed);
            if let Some(s) = slot_in_a.lock().unwrap().take() {
                s.unsubscribe();
            }
        }),
    );
    let b = hub.subscribe(1, cb_b);
    *b_slot.lock().unwrap() = Some(b);

    p.publish(&[1]);
    assert_eq!(take(&rx_self, 1), [value(1, ValueKind::U8, &[1])]);
    assert_eq!(a_hits.load(Ordering::Relaxed), 1);
    assert!(
        drain(&rx_b).is_empty(),
        "skipped in the dispatch that unsubscribed it"
    );
    p.publish(&[2]);
    assert!(drain(&rx_self).is_empty(), "the next delivery misses");
    assert_eq!(a_hits.load(Ordering::Relaxed), 2);
    assert!(drain(&rx_b).is_empty());
}

#[test]
fn publishing_another_id_inside_a_callback_works() {
    let hub = Hub::new();
    let p1 = hub.publisher(1, ValueKind::U8);
    let p2 = hub.publisher(2, ValueKind::U8);
    let (cb2, rx2) = sink();
    let _s2 = hub.subscribe(2, cb2);
    let _s1 = hub.subscribe(1, Arc::new(move |_, _| p2.publish(&[42])));
    p1.publish(&[1]);
    assert_eq!(take(&rx2, 1), [value(2, ValueKind::U8, &[42])]);
}

#[test]
fn get_inside_a_callback_returns_the_value_being_delivered() {
    let hub = Hub::new();
    let p = hub.publisher(1, ValueKind::U32);
    let seen = Arc::new(Mutex::new(Vec::new()));
    let (hub_in_cb, seen_in_cb) = (hub.clone(), Arc::clone(&seen));
    let _s = hub.subscribe(
        1,
        Arc::new(move |id, _| {
            let v = u32::from_le_bytes(hub_in_cb.get(id).unwrap().1.as_ref().try_into().unwrap());
            seen_in_cb.lock().unwrap().push(v);
        }),
    );
    for v in [1u32, 2] {
        p.publish(&v.to_le_bytes());
    }
    assert_eq!(*seen.lock().unwrap(), [1, 2]);
}

#[test]
fn concurrent_use_has_no_deadlock_and_delivers_everything() {
    const IDS: u32 = 4;
    const PER_THREAD: usize = 300;
    let hub = Hub::new();
    let publishers: Vec<Publisher> = (0..IDS)
        .map(|id| hub.publisher(id, ValueKind::U8))
        .collect();
    let counts: Vec<Arc<AtomicUsize>> = (0..IDS).map(|_| Arc::new(AtomicUsize::new(0))).collect();
    let subs: Vec<Subscription> = counts
        .iter()
        .enumerate()
        .map(|(id, c)| {
            let c = Arc::clone(c);
            hub.subscribe(
                id as u32,
                Arc::new(move |_, ev| {
                    if matches!(ev, Event::Value(..)) {
                        c.fetch_add(1, Ordering::Relaxed);
                    }
                }),
            )
        })
        .collect();

    thread::scope(|scope| {
        for p in &publishers {
            scope.spawn(move || {
                for i in 0..PER_THREAD {
                    p.publish(&[i as u8]);
                }
            });
        }
        for _ in 0..2 {
            let hub = &hub;
            scope.spawn(move || {
                for i in 0..PER_THREAD {
                    let id = i as u32 % IDS;
                    let p = hub.publisher(id, ValueKind::U8);
                    let s = hub.subscribe(id, Arc::new(|_, _| {}));
                    s.unsubscribe();
                    p.publish(&[0]);
                    drop(p);
                }
            });
        }
    });

    // own thread plus the two churn threads, which visit every id equally often
    let expected = PER_THREAD + 2 * PER_THREAD / IDS as usize;
    for (id, c) in counts.iter().enumerate() {
        assert_eq!(c.load(Ordering::Relaxed), expected, "id {id}");
    }
    drop(subs);
}

/// A one-thread executor: `scheduler()` hands jobs to its thread. Drop every subscription
/// holding one of its schedulers before `finish()`, or the thread never sees the channel close.
struct Executor {
    id: thread::ThreadId,
    tx: Arc<Mutex<mpsc::Sender<Job>>>,
    handle: Option<thread::JoinHandle<()>>,
}

type Job = Box<dyn FnOnce() + Send>;

impl Executor {
    fn new() -> Self {
        let (tx, rx) = mpsc::channel::<Job>();
        let handle = thread::spawn(move || {
            while let Ok(job) = rx.recv() {
                job();
            }
        });
        Executor {
            id: handle.thread().id(),
            tx: Arc::new(Mutex::new(tx)),
            handle: Some(handle),
        }
    }

    fn scheduler(&self) -> Scheduler {
        let tx = Arc::clone(&self.tx);
        Arc::new(move |job: Job| tx.lock().unwrap().send(job).unwrap())
    }

    fn finish(mut self) {
        let handle = self.handle.take().unwrap();
        drop(self.tx);
        handle.join().unwrap();
    }
}

#[test]
fn subscribe_on_runs_each_subscriber_on_its_own_executor() {
    let hub = Hub::new();
    let p = hub.publisher(1, ValueKind::U8);
    let a = Executor::new();
    let b = Executor::new();

    // every delivery records the thread it ran on
    let (tx, rx) = mpsc::channel::<(thread::ThreadId, Item)>();
    let tx = Mutex::new(tx);
    let record: Callback = Arc::new(move |id, ev| {
        tx.lock()
            .unwrap()
            .send((thread::current().id(), item(id, ev)))
            .unwrap();
    });
    let _sa = hub.subscribe_on(1, Arc::clone(&record), a.scheduler());
    let _sb = hub.subscribe_on(1, Arc::clone(&record), b.scheduler());
    let _inline = hub.subscribe(1, Arc::clone(&record));

    let by_thread = |n: usize| {
        let mut seen: HashMap<thread::ThreadId, Vec<Item>> = HashMap::new();
        for (t, it) in take(&rx, n) {
            seen.entry(t).or_default().push(it);
        }
        seen
    };
    let on_publisher = thread::current().id();

    p.publish(&[5]);
    assert_eq!(
        hub.get(1).unwrap().1.as_ref(),
        [5],
        "the value is recorded synchronously"
    );
    let seen = by_thread(3);
    assert_eq!(
        seen.len(),
        3,
        "one delivery per subscriber, each on its own thread"
    );
    for t in [&a.id, &b.id, &on_publisher] {
        assert_eq!(seen[t], [value(1, ValueKind::U8, &[5])]);
    }

    // Closed follows each subscriber's executor too
    drop(p);
    let seen = by_thread(3);
    assert_eq!(seen.len(), 3);
    for t in [&a.id, &b.id, &on_publisher] {
        assert_eq!(seen[t], [(1, None)]);
    }

    drop((_sa, _sb, _inline));
    a.finish();
    b.finish();
}

#[test]
fn value_round_trips_and_rejects_bad_input() {
    let values = [
        Value::Bool(true),
        Value::U8(7),
        Value::I8(-1),
        Value::U16(300),
        Value::I16(-300),
        Value::U32(70_000),
        Value::I32(-70_000),
        Value::U64(1 << 42),
        Value::I64(-(1 << 42)),
        Value::F32(1.5),
        Value::F64(-2.25),
        Value::String("héllo 世界".into()),
        Value::Bytes(vec![0, 255, 7]),
        Value::Message(vec![1, 0, 0, 0, 24]),
    ];
    for (i, v) in values.iter().enumerate() {
        assert_eq!(v.kind(), ValueKind::from_u8(i as u8).unwrap(), "{v:?}");
        assert_eq!(Value::decode(v.kind(), &v.encode()).unwrap(), *v);
    }
    assert_eq!(ValueKind::from_u8(14), None);
    assert_eq!(ValueKind::U32.fixed_len(), Some(4));
    assert_eq!(ValueKind::Bool.fixed_len(), Some(1));
    assert_eq!(ValueKind::String.fixed_len(), None);
    assert_eq!(
        Value::decode(ValueKind::U32, &[1, 2]),
        Err(ValueError("wrong payload length"))
    );
    assert_eq!(
        Value::decode(ValueKind::Bool, &[]),
        Err(ValueError("wrong payload length"))
    );
    assert_eq!(
        Value::decode(ValueKind::F64, &[0; 9]),
        Err(ValueError("wrong payload length"))
    );
    assert_eq!(
        Value::decode(ValueKind::String, &[0xFF]),
        Err(ValueError("invalid utf-8"))
    );
    assert_eq!(
        Value::decode(ValueKind::Bool, &[2]).unwrap(),
        Value::Bool(true)
    );
    assert_eq!(
        Value::decode(ValueKind::Bytes, b"raw").unwrap(),
        Value::Bytes(b"raw".to_vec())
    );
    assert_eq!(
        Value::decode(ValueKind::String, "héllo".as_bytes()).unwrap(),
        Value::String("héllo".into())
    );
}

#[test]
fn handles_survive_the_hub() {
    let hub = Hub::new();
    let p = hub.publisher(1, ValueKind::U8);
    let (cb, rx) = sink();
    let s = hub.subscribe(1, cb);
    drop(hub);
    p.publish(&[1]);
    p.publish_value(&Value::U8(2)).unwrap();
    p.close();
    s.unsubscribe();
    drop((p, s));
    assert!(drain(&rx).is_empty());
}

#[test]
fn kind_is_fixed_per_publisher() {
    let hub = Hub::new();
    let a = hub.publisher(1, ValueKind::U32);
    let b = hub.publisher(1, ValueKind::String);
    assert_eq!((a.id(), a.kind()), (1, ValueKind::U32));
    assert_eq!(b.kind(), ValueKind::String);
    assert_eq!(
        a.publish_value(&Value::String("x".into())),
        Err(ValueError("kind mismatch"))
    );
    a.publish_value(&Value::U32(5)).unwrap();
    assert_eq!(hub.get(1).unwrap().0, ValueKind::U32);
    b.publish_value(&Value::String("x".into())).unwrap();
    assert_eq!(hub.get(1), Some((ValueKind::String, Arc::from(&b"x"[..]))));
    assert_eq!(hub.get_value(1), Some(Ok(Value::String("x".into()))));
}

#[test]
fn pull_needs_a_live_publisher_and_answers_with_the_newest_publish() {
    let hub = Hub::new();
    assert_eq!(hub.get(1), None);
    let p1 = hub.publisher(1, ValueKind::U32);
    assert_eq!(hub.get(1), None, "never published");
    p1.publish(&1u32.to_le_bytes());
    let p2 = hub.publisher(1, ValueKind::U32);
    p2.publish(&2u32.to_le_bytes());
    assert_eq!(hub.get(1).unwrap().1.as_ref(), 2u32.to_le_bytes());
    p2.close();
    assert_eq!(
        hub.get(1).unwrap().1.as_ref(),
        1u32.to_le_bytes(),
        "falls back to the remaining publisher"
    );
    drop(p1);
    assert_eq!(hub.get(1), None);
    let _p3 = hub.publisher(1, ValueKind::U32);
    assert_eq!(hub.get(1), None, "a new publisher has no value yet");
}

#[test]
fn closing_the_last_publisher_delivers_closed_once_after_every_value() {
    let hub = Hub::new();
    let p1 = hub.publisher(1, ValueKind::U8);
    let p2 = hub.publisher(1, ValueKind::U8);
    let (cb, rx) = sink();
    let _s = hub.subscribe(1, cb);
    p1.publish(&[1]);
    p2.publish(&[2]);
    p1.close();
    assert_eq!(drain(&rx).len(), 2, "a non-last close delivers nothing");
    p2.close();
    assert_eq!(take(&rx, 1), [(1, None)]);
    assert!(drain(&rx).is_empty());
}

#[test]
fn publishing_after_close_is_a_noop_and_close_is_idempotent() {
    let hub = Hub::new();
    let p = hub.publisher(1, ValueKind::U8);
    let (cb, rx) = sink();
    let _s = hub.subscribe(1, cb);
    p.close();
    p.close();
    p.publish(&[1]);
    p.publish_value(&Value::U8(1)).unwrap();
    drop(p);
    assert_eq!(take(&rx, 1), [(1, None)], "Closed exactly once");
    assert!(drain(&rx).is_empty());
    assert_eq!(hub.get(1), None);
}

#[test]
fn closing_inside_a_callback_is_allowed_and_dispatches_recursively() {
    let hub = Hub::new();

    // closing the last publisher of another id inside a callback delivers its Closed recursively
    let pa = hub.publisher(1, ValueKind::U8);
    let pb = hub.publisher(2, ValueKind::U8);
    let (cb1, rx1) = sink();
    let (cb2, rx2) = sink();
    let _s2 = hub.subscribe(2, cb2);
    let _s1 = hub.subscribe(
        1,
        Arc::new(move |id, ev| {
            cb1(id, ev);
            pb.close();
        }),
    );
    pa.publish(&[1]);
    assert_eq!(take(&rx1, 1), [value(1, ValueKind::U8, &[1])]);
    assert_eq!(take(&rx2, 1), [(2, None)]);
    assert_eq!(hub.get(2), None);

    // closing a non-last publisher of the same id inside its own dispatch delivers nothing;
    // the values still arrive in order and Closed only follows the last one
    let p3a = hub.publisher(3, ValueKind::U8);
    let p3b = hub.publisher(3, ValueKind::U8);
    let (cb3, rx3) = sink();
    let _s3 = hub.subscribe(
        3,
        Arc::new(move |id, ev| {
            cb3(id, ev);
            p3b.close();
        }),
    );
    p3a.publish(&[1]);
    assert_eq!(drain(&rx3), [value(3, ValueKind::U8, &[1])]);
    p3a.publish(&[2]);
    p3a.close();
    assert_eq!(
        drain(&rx3),
        [value(3, ValueKind::U8, &[2]), (3, None)],
        "Closed after every value, once"
    );
}

#[test]
fn subscriptions_survive_closed_and_reach_a_new_publisher() {
    let hub = Hub::new();
    let p1 = hub.publisher(1, ValueKind::U8);
    let (cb, rx) = sink();
    let _s = hub.subscribe(1, cb);
    p1.publish(&[1]);
    drop(p1);
    let p2 = hub.publisher(1, ValueKind::U8);
    p2.publish(&[2]);
    assert_eq!(
        take(&rx, 3),
        [
            value(1, ValueKind::U8, &[1]),
            (1, None),
            value(1, ValueKind::U8, &[2])
        ]
    );
}

#[test]
fn shared_is_one_registry_per_process() {
    // the only test touching the shared hub, so an id of its own keeps it independent
    const ID: DataId = 0x51A2B;
    assert!(std::ptr::eq(Hub::shared(), Hub::shared()));
    let (cb, rx) = sink();
    let _s = Hub::shared().subscribe(ID, cb);
    let p = Hub::shared().publisher(ID, ValueKind::U8);
    p.publish(&[1]);
    assert_eq!(take(&rx, 1), [value(ID, ValueKind::U8, &[1])]);
    assert_eq!(Hub::shared().get(ID).unwrap().1.as_ref(), [1]);
    assert_eq!(
        Hub::new().get(ID),
        None,
        "a fresh hub is an isolated registry"
    );
}
