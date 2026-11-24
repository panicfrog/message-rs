//! In-process pub/sub data layer: one registry behind a lock, explicit `Publisher` and
//! `Subscription` handles, dispatch outside the lock on snapshots. `DESIGN.md` has the rules.

mod value;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, Weak};

pub use value::{Value, ValueError, ValueKind};

pub type DataId = u32;

/// What a subscriber receives: a published value, or the end of every publisher of its id.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event<'a> {
    Value(ValueKind, &'a [u8]),
    Closed,
}

pub type Callback = Arc<dyn Fn(DataId, Event<'_>) + Send + Sync>;

/// Runs a `subscribe_on` subscriber's jobs on its own loop.
pub type Scheduler = Arc<dyn Fn(Box<dyn FnOnce() + Send>) + Send + Sync>;

/// The registry. Clones share it.
#[derive(Clone)]
pub struct Hub {
    inner: Arc<Inner>,
}

/// RAII publishing handle bound to one `(id, kind)`; `Drop` closes it.
pub struct Publisher {
    hub: Weak<Inner>,
    id: DataId,
    kind: ValueKind,
    token: u64,
}

/// RAII subscription; `Drop` unsubscribes.
pub struct Subscription {
    hub: Weak<Inner>,
    id: DataId,
    sub: Arc<SubState>,
}

struct Inner {
    state: Mutex<State>,
}

struct State {
    ids: HashMap<DataId, Entry>,
    seq: u64,
    next_token: u64,
}

#[derive(Default)]
struct Entry {
    subs: Vec<Arc<SubState>>,
    pubs: Vec<PubRec>,
}

/// Shared with the `Subscription` handle, so a delivery can skip an unsubscribed callback
/// without the registry lock. `scheduler` is per subscriber: one hub serves several loops.
struct SubState {
    cb: Callback,
    alive: AtomicBool,
    scheduler: Option<Scheduler>,
}

/// One live publisher as the registry sees it; `seq` is the stamp of its last publish.
struct PubRec {
    token: u64,
    kind: ValueKind,
    seq: u64,
    last: Option<Arc<[u8]>>,
}

/// Owned delivery: keeps the payload alive for a scheduled job.
#[derive(Clone)]
enum Delivery {
    Value(ValueKind, Arc<[u8]>),
    Closed,
}

impl Default for Hub {
    fn default() -> Self {
        Self::new()
    }
}

impl Hub {
    pub fn new() -> Self {
        Hub {
            inner: Arc::new(Inner {
                state: Mutex::new(State {
                    ids: HashMap::new(),
                    seq: 0,
                    next_token: 0,
                }),
            }),
        }
    }

    /// The one registry every end of a process shares; `new()` is for isolated registries.
    pub fn shared() -> &'static Hub {
        static SHARED: OnceLock<Hub> = OnceLock::new();
        SHARED.get_or_init(Hub::new)
    }

    pub fn publisher(&self, id: DataId, kind: ValueKind) -> Publisher {
        let token = {
            let mut guard = self.inner.lock();
            let st = &mut *guard;
            st.next_token += 1;
            st.ids.entry(id).or_default().pubs.push(PubRec {
                token: st.next_token,
                kind,
                seq: 0,
                last: None,
            });
            st.next_token
        };
        Publisher {
            hub: Arc::downgrade(&self.inner),
            id,
            kind,
            token,
        }
    }

    /// Deliveries run inline on the publisher's thread.
    pub fn subscribe(&self, id: DataId, cb: Callback) -> Subscription {
        self.subscribe_inner(id, cb, None)
    }

    /// Like `subscribe`, but every delivery is handed to `s` as a job instead of running inline.
    pub fn subscribe_on(&self, id: DataId, cb: Callback, s: Scheduler) -> Subscription {
        self.subscribe_inner(id, cb, Some(s))
    }

    fn subscribe_inner(
        &self,
        id: DataId,
        cb: Callback,
        scheduler: Option<Scheduler>,
    ) -> Subscription {
        let sub = Arc::new(SubState {
            cb,
            alive: AtomicBool::new(true),
            scheduler,
        });
        self.inner
            .lock()
            .ids
            .entry(id)
            .or_default()
            .subs
            .push(Arc::clone(&sub));
        Subscription {
            hub: Arc::downgrade(&self.inner),
            id,
            sub,
        }
    }

    /// Newest published value among the id's live publishers.
    pub fn get(&self, id: DataId) -> Option<(ValueKind, Arc<[u8]>)> {
        let st = self.inner.lock();
        let (_, kind, last) = st
            .ids
            .get(&id)?
            .pubs
            .iter()
            .filter_map(|p| p.last.as_ref().map(|last| (p.seq, p.kind, last)))
            .max_by_key(|&(seq, _, _)| seq)?;
        Some((kind, Arc::clone(last)))
    }

    pub fn get_value(&self, id: DataId) -> Option<Result<Value, ValueError>> {
        self.get(id)
            .map(|(kind, bytes)| Value::decode(kind, &bytes))
    }
}

impl Inner {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().expect("hub state poisoned")
    }

    /// Subscriber snapshot for `id`, taken under a lock the caller already holds so that it is
    /// atomic with the registry update that caused the delivery.
    fn snapshot(st: &State, id: DataId) -> Vec<Arc<SubState>> {
        st.ids.get(&id).map(|e| e.subs.clone()).unwrap_or_default()
    }
}

/// Deliver to a snapshot outside the lock: inline, or as one job per subscriber through its own
/// scheduler. Subscribers unsubscribed before their delivery runs are skipped.
fn dispatch(id: DataId, subs: Vec<Arc<SubState>>, d: Delivery) {
    for sub in subs {
        let scheduler = sub.scheduler.clone();
        let job: Box<dyn FnOnce() + Send> = {
            let d = d.clone();
            Box::new(move || {
                if !sub.alive.load(Ordering::Acquire) {
                    return;
                }
                match d {
                    Delivery::Value(kind, bytes) => {
                        (sub.cb)(id, Event::Value(kind, bytes.as_ref()))
                    }
                    Delivery::Closed => (sub.cb)(id, Event::Closed),
                }
            })
        };
        match scheduler {
            Some(s) => s(job),
            None => job(),
        }
    }
}

impl Publisher {
    pub fn id(&self) -> DataId {
        self.id
    }

    pub fn kind(&self) -> ValueKind {
        self.kind
    }

    pub fn publish(&self, payload: &[u8]) {
        let Some(inner) = self.hub.upgrade() else {
            return;
        };
        let bytes: Arc<[u8]> = Arc::from(payload);
        let subs = {
            let mut guard = inner.lock();
            let st = &mut *guard;
            let Some(rec) = st
                .ids
                .get_mut(&self.id)
                .and_then(|e| e.pubs.iter_mut().find(|p| p.token == self.token))
            else {
                return;
            };
            st.seq += 1;
            rec.seq = st.seq;
            rec.last = Some(Arc::clone(&bytes));
            Inner::snapshot(st, self.id)
        };
        dispatch(self.id, subs, Delivery::Value(self.kind, bytes));
    }

    pub fn publish_value(&self, v: &Value) -> Result<(), ValueError> {
        if v.kind() != self.kind {
            return Err(ValueError("kind mismatch"));
        }
        self.publish(&v.encode());
        Ok(())
    }

    pub fn close(&self) {
        let Some(inner) = self.hub.upgrade() else {
            return;
        };
        let mut st = inner.lock();
        let Some(e) = st.ids.get_mut(&self.id) else {
            return;
        };
        let Some(i) = e.pubs.iter().position(|p| p.token == self.token) else {
            return;
        };
        e.pubs.remove(i);
        if !e.pubs.is_empty() {
            return;
        }
        let subs = if e.subs.is_empty() {
            st.ids.remove(&self.id);
            Vec::new()
        } else {
            Inner::snapshot(&st, self.id)
        };
        drop(st);
        dispatch(self.id, subs, Delivery::Closed);
    }
}

impl Drop for Publisher {
    fn drop(&mut self) {
        self.close();
    }
}

impl Subscription {
    pub fn id(&self) -> DataId {
        self.id
    }

    pub fn unsubscribe(&self) {
        self.sub.alive.store(false, Ordering::Release);
        let Some(inner) = self.hub.upgrade() else {
            return;
        };
        let mut st = inner.lock();
        let Some(e) = st.ids.get_mut(&self.id) else {
            return;
        };
        e.subs.retain(|s| !Arc::ptr_eq(s, &self.sub));
        if e.subs.is_empty() && e.pubs.is_empty() {
            st.ids.remove(&self.id);
        }
    }
}

impl Drop for Subscription {
    fn drop(&mut self) {
        self.unsubscribe();
    }
}
