//! The cxx bridge over `message-hub`. Every function is infallible, so no C++ exception ever
//! crosses a Swift frame; handles are raw pointers to opaque types and the caller owns them
//! (`release`, or `Drop` in the Swift wrapper).
//!
//! Two shapes are dictated by what Swift's C++ interop can express (see hub/DESIGN.md, M2):
//! byte buffers cross as `(ptr, len)` (Swift cannot construct `rust::Slice`), and callbacks
//! cross as opaque addresses (`*const u8`) that this side transmutes back to `extern "C"`
//! functions (cxx does not take function pointers from C++ to Rust).

#[cxx::bridge(namespace = "message_hub")]
pub mod ffi {
    extern "Rust" {
        /// One registry. `shared()` is the process-wide instance; `create()` an isolated one.
        type Hub;
        /// RAII publishing handle bound to one `(id, kind)`.
        type Publisher;
        /// RAII subscription handle.
        type Subscription;

        fn shared() -> &'static Hub;
        fn create() -> *mut Hub;
        unsafe fn release_hub(h: *mut Hub);

        /// Invalid kinds yield null: the caller keeps a stable failure mode instead of a panic.
        unsafe fn publisher(h: &Hub, id: u32, kind_u8: u8) -> *mut Publisher;
        unsafe fn publisher_id(p: &Publisher) -> u32;
        unsafe fn publisher_kind(p: &Publisher) -> u8;
        /// A no-op after `close`. Payload length is not checked here: the hub is payload-agnostic,
        /// fixed-size kinds are validated when a value is decoded.
        unsafe fn publish(p: &Publisher, ptr: *const u8, len: usize);
        unsafe fn close(p: &Publisher);
        unsafe fn release_publisher(p: *mut Publisher);

        /// `Closed` is delivered as `ptr = null, len = 0` (see `CLOSED_KIND`). `ctx_drop` frees
        /// `ctx` once nothing references it any more (not at `unsubscribe`: a scheduled job may
        /// still be pending).
        unsafe fn subscribe(h: &Hub, id: u32, cb: *const u8, ctx: *mut u8, ctx_drop: *const u8) -> *mut Subscription;
        /// Like `subscribe`, but each delivery is handed to `schedule` as a job.
        unsafe fn subscribe_on(
            h: &Hub,
            id: u32,
            cb: *const u8,
            ctx: *mut u8,
            ctx_drop: *const u8,
            schedule: *const u8,
            schedule_ctx: *mut u8,
        ) -> *mut Subscription;
        unsafe fn subscription_id(s: &Subscription) -> u32;
        unsafe fn unsubscribe(s: &Subscription);
        unsafe fn release_subscription(s: *mut Subscription);

        /// The newest value of `id` as `(kind, bytes)`; empty bytes mean "nothing published".
        /// `kind` is written through the out pointer only when a value is returned.
        unsafe fn get(h: &Hub, id: u32, kind_out: &mut u8) -> Vec<u8>;
    }
}

// ---- Rust side of the bridge (cxx looks the `extern "Rust"` items up in this module) ----

use message_hub::{Callback, Event};
use std::sync::Arc;

/// Opaque handles. cxx requires them to be defined in this crate, so each wraps the core's.
/// They are only ever handled through raw pointers; the caller owns them.
pub struct Hub(message_hub::Hub);
pub struct Publisher(message_hub::Publisher);
pub struct Subscription(message_hub::Subscription);

/// The `kind_u8` a `Closed` event carries; outside `ValueKind`'s range on purpose.
pub const CLOSED_KIND: u8 = 255;

/// `ctx, id, kind_u8, ptr, len` — called on the publisher's thread (or the scheduler's).
pub type Trampoline = unsafe extern "C" fn(ctx: *mut u8, id: u32, kind_u8: u8, ptr: *const u8, len: usize);
/// Frees the caller's context once no delivery can use it any more.
pub type CtxDrop = unsafe extern "C" fn(ctx: *mut u8);
/// `schedule_ctx, job_ctx, run, drop` — queues the job, later calls `run(job_ctx)` on its own
/// loop and finally `drop(job_ctx)` once to free it.
pub type Schedule = unsafe extern "C" fn(schedule_ctx: *mut u8, job_ctx: *mut u8, run: *const u8, drop: *const u8);

/// Everything the foreign side registered for one subscription, behind an `Arc` that every
/// queued job shares. Consequence for the caller: `ctx` is freed when the *last* reference goes
/// away — on `release_subscription` for inline subscribers, after the pending jobs of a
/// scheduled one have run — and must not be touched by the caller once it handed it over.
struct ForeignSub {
    cb: Trampoline,
    ctx: *mut u8,
    ctx_drop: CtxDrop,
    schedule: Option<(Schedule, *mut u8)>,
}

// The foreign context is only ever touched through its C functions, from any thread.
unsafe impl Send for ForeignSub {}
unsafe impl Sync for ForeignSub {}

impl Drop for ForeignSub {
    fn drop(&mut self) {
        unsafe { (self.ctx_drop)(self.ctx) }
    }
}

/// A scheduled job: the payload (kept alive by its own `Arc`) plus the target subscriber.
struct Job {
    sub: Arc<ForeignSub>,
    id: u32,
    kind_u8: u8,
    bytes: Option<Arc<[u8]>>,
}

/// Runs the delivery; the job itself stays owned by the scheduler until it calls `drop_job`.
extern "C" fn run_job(job_ctx: *mut u8) {
    unsafe {
        let job = &*(job_ctx as *const Job);
        deliver(&job.sub, job.id, job.kind_u8, job.bytes.as_deref());
    }
}

extern "C" fn drop_job(job_ctx: *mut u8) {
    unsafe {
        drop(Box::from_raw(job_ctx as *mut Job));
    }
}

fn deliver(sub: &ForeignSub, id: u32, kind_u8: u8, bytes: Option<&[u8]>) {
    // `Closed` is the one event without a payload: a null pointer, length 0.
    let (ptr, len) = match bytes {
        Some(b) => (b.as_ptr(), b.len()),
        None => (std::ptr::null(), 0),
    };
    unsafe { (sub.cb)(sub.ctx, id, kind_u8, ptr, len) }
}

/// The hub-facing callback: runs the delivery inline, or packs it into a `Job` the foreign
/// scheduler owns (`run` once, `drop` once).
fn make_callback(sub: Arc<ForeignSub>) -> Callback {
    Arc::new(move |id, event: Event<'_>| {
        let (kind_u8, bytes) = match event {
            Event::Value(kind, b) => (kind as u8, Some(Arc::from(b))),
            Event::Closed => (CLOSED_KIND, None),
        };
        match sub.schedule {
            None => deliver(&sub, id, kind_u8, bytes.as_deref()),
            Some((schedule, schedule_ctx)) => {
                let job = Box::new(Job { sub: Arc::clone(&sub), id, kind_u8, bytes });
                unsafe { schedule(schedule_ctx, Box::into_raw(job) as *mut u8, run_job as *const u8, drop_job as *const u8) }
            }
        }
    })
}

fn register(h: &Hub, id: u32, cb: *const u8, ctx: *mut u8, ctx_drop: *const u8, schedule: Option<(*const u8, *mut u8)>) -> *mut Subscription {
    // Invalid addresses are a programming error on the caller's side; keep the failure local
    // instead of transmuting garbage into a function pointer.
    assert!(!cb.is_null() && !ctx_drop.is_null(), "subscribe needs a callback and a ctx drop fn");
    let schedule = schedule.map(|(s, c)| {
        assert!(!s.is_null(), "subscribe_on needs a schedule fn");
        (unsafe { std::mem::transmute::<*const u8, Schedule>(s) }, c)
    });
    let sub = Arc::new(ForeignSub {
        cb: unsafe { std::mem::transmute::<*const u8, Trampoline>(cb) },
        ctx,
        ctx_drop: unsafe { std::mem::transmute::<*const u8, CtxDrop>(ctx_drop) },
        schedule,
    });
    let callback = make_callback(Arc::clone(&sub));
    // Scheduled deliveries never run on the publisher's thread, so the hub's own scheduler
    // hook is not needed: the callback hands the job over itself.
    let subscription = h.0.subscribe(id, callback);
    Box::into_raw(Box::new(Subscription(subscription))) as *mut Subscription
}

/// The process-wide registry, leaked for the life of the process (the core keeps it in a
/// static; the box gives the FFI a stable address that is never freed).
pub fn shared() -> &'static Hub {
    static SHARED: std::sync::OnceLock<Hub> = std::sync::OnceLock::new();
    SHARED.get_or_init(|| Hub(message_hub::Hub::shared().clone()))
}

pub fn create() -> *mut Hub {
    Box::into_raw(Box::new(Hub(message_hub::Hub::new())))
}

pub unsafe fn release_hub(h: *mut Hub) {
    if !h.is_null() {
        drop(Box::from_raw(h));
    }
}

pub unsafe fn publisher(h: &Hub, id: u32, kind_u8: u8) -> *mut Publisher {
    match message_hub::ValueKind::from_u8(kind_u8) {
        Some(kind) => Box::into_raw(Box::new(Publisher(h.0.publisher(id, kind)))),
        None => std::ptr::null_mut(),
    }
}

pub unsafe fn publisher_id(p: &Publisher) -> u32 {
    p.0.id()
}

pub unsafe fn publisher_kind(p: &Publisher) -> u8 {
    p.0.kind() as u8
}

pub unsafe fn publish(p: &Publisher, ptr: *const u8, len: usize) {
    let bytes: &[u8] = if len == 0 { &[] } else { std::slice::from_raw_parts(ptr, len) };
    p.0.publish(bytes);
}

pub unsafe fn close(p: &Publisher) {
    p.0.close();
}

pub unsafe fn release_publisher(p: *mut Publisher) {
    if !p.is_null() {
        drop(Box::from_raw(p)); // Drop closes
    }
}

pub unsafe fn subscribe(h: &Hub, id: u32, cb: *const u8, ctx: *mut u8, ctx_drop: *const u8) -> *mut Subscription {
    register(h, id, cb, ctx, ctx_drop, None)
}

pub unsafe fn subscribe_on(
    h: &Hub,
    id: u32,
    cb: *const u8,
    ctx: *mut u8,
    ctx_drop: *const u8,
    schedule: *const u8,
    schedule_ctx: *mut u8,
) -> *mut Subscription {
    register(h, id, cb, ctx, ctx_drop, Some((schedule, schedule_ctx)))
}

pub unsafe fn subscription_id(s: &Subscription) -> u32 {
    s.0.id()
}

pub unsafe fn unsubscribe(s: &Subscription) {
    s.0.unsubscribe();
}

pub unsafe fn release_subscription(s: *mut Subscription) {
    if !s.is_null() {
        drop(Box::from_raw(s)); // Drop unsubscribes
    }
}

pub unsafe fn get(h: &Hub, id: u32, kind_out: &mut u8) -> Vec<u8> {
    match h.0.get(id) {
        Some((kind, bytes)) => {
            *kind_out = kind as u8;
            bytes.to_vec()
        }
        None => Vec::new(),
    }
}
