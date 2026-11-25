//! The bridge driven exactly the way Swift will drive it: `extern "C"` trampolines, opaque
//! addresses cast back and forth, contexts released through a drop function. No cxx runtime is
//! involved on this side, so the tests also pin the ABI-level contract.

use hub_ffi::bridge::*;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

/// A context the way Swift's `CallbackBox` will be: heap-allocated, freed by a C fn.
struct Ctx {
    events: Mutex<Vec<(u32, u8, Vec<u8>)>>,
    drops: Arc<AtomicUsize>,
}

unsafe extern "C" fn cb(ctx: *mut u8, id: u32, kind_u8: u8, ptr: *const u8, len: usize) {
    let ctx = &*(ctx as *const Ctx);
    let bytes = if ptr.is_null() { Vec::new() } else { std::slice::from_raw_parts(ptr, len).to_vec() };
    ctx.events.lock().unwrap().push((id, kind_u8, bytes));
}

unsafe extern "C" fn drop_ctx(ctx: *mut u8) {
    let ctx = Box::from_raw(ctx as *mut Ctx);
    ctx.drops.fetch_add(1, Ordering::SeqCst);
}

/// A queue of jobs a "foreign run loop" would drain later.
struct Loop {
    jobs: Mutex<Vec<(*mut u8, usize, usize)>>,
}

unsafe extern "C" fn schedule(schedule_ctx: *mut u8, job_ctx: *mut u8, run: *const u8, drop: *const u8) {
    let r#loop = &*(schedule_ctx as *const Loop);
    r#loop.jobs.lock().unwrap().push((job_ctx, run as usize, drop as usize));
}

impl Loop {
    fn drain(&self) {
        let jobs: Vec<_> = std::mem::take(&mut *self.jobs.lock().unwrap());
        for (job_ctx, run, drop) in jobs {
            unsafe {
                let run: unsafe extern "C" fn(*mut u8) = std::mem::transmute(run);
                let drop: unsafe extern "C" fn(*mut u8) = std::mem::transmute(drop);
                run(job_ctx);
                drop(job_ctx);
            }
        }
    }
}

struct Fixture {
    hub: *mut Hub,
    drops: Arc<AtomicUsize>,
}

impl Fixture {
    fn new() -> Self {
        Fixture { hub: create(), drops: Arc::new(AtomicUsize::new(0)) }
    }

    fn ctx(&self) -> *mut u8 {
        Box::into_raw(Box::new(Ctx { events: Mutex::new(Vec::new()), drops: self.drops.clone() })) as *mut u8
    }

    fn ctx_of(&self, ctx: *mut u8) -> &Ctx {
        unsafe { &*(ctx as *const Ctx) }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        unsafe { release_hub(self.hub) }
    }
}

unsafe fn publish_bytes(p: &Publisher, bytes: &[u8]) {
    publish(p, bytes.as_ptr(), bytes.len())
}

#[test]
fn shared_is_one_registry_across_calls() {
    let a = shared() as *const Hub;
    let b = shared() as *const Hub;
    assert_eq!(a, b);
    assert_eq!(a as usize % std::mem::align_of::<Hub>(), 0);
}

#[test]
fn publish_subscribe_get_through_the_bridge() {
    let f = Fixture::new();
    let ctx = f.ctx();
    unsafe {
        let p = publisher(&*f.hub, 1, 13);
        assert!(!p.is_null());
        assert_eq!(publisher_id(&*p), 1);
        assert_eq!(publisher_kind(&*p), 13);

        // pull before any publish: empty
        let mut kind = 0u8;
        assert!(get(&*f.hub, 1, &mut kind).is_empty());

        let s = subscribe(&*f.hub, 1, cb as *const u8, ctx, drop_ctx as *const u8);
        assert!(!s.is_null());
        assert_eq!(subscription_id(&*s), 1);

        publish_bytes(&*p, b"hello");
        publish_bytes(&*p, b"world");

        let got = get(&*f.hub, 1, &mut kind);
        assert_eq!((kind, got.as_slice()), (13, b"world".as_slice()));

        let events = f.ctx_of(ctx).events.lock().unwrap().clone();
        assert_eq!(
            events,
            vec![(1, 13, b"hello".to_vec()), (1, 13, b"world".to_vec())],
            "pure forwarding: every publish, in order, nothing replayed"
        );

        // Releasing the subscription frees the context, so the "no more deliveries" check
        // reads the drop counter instead of the (now dangling) context.
        release_subscription(s);
        assert_eq!(f.drops.load(Ordering::SeqCst), 1, "the subscription owned the context");
        publish_bytes(&*p, b"after");
        assert_eq!(f.drops.load(Ordering::SeqCst), 1, "exactly one free: nothing referenced it after release");
    }
}

#[test]
fn invalid_kinds_yield_null_and_the_kind_travels_with_get() {
    let f = Fixture::new();
    unsafe {
        let u32p = publisher(&*f.hub, 7, 5);
        assert!(!u32p.is_null());
        publish_bytes(&*u32p, &[1, 0, 0, 0]);
        let mut kind = 0u8;
        let bytes = get(&*f.hub, 7, &mut kind);
        assert_eq!(bytes, vec![1, 0, 0, 0]);
        assert_eq!(kind, 5, "get reports the publisher's kind");

        assert!(publisher(&*f.hub, 8, 200).is_null(), "unknown kind");
        assert!(publisher(&*f.hub, 8, CLOSED_KIND).is_null(), "the Closed marker is not a kind");
        release_publisher(u32p);
    }
}

#[test]
fn closing_the_last_publisher_delivers_closed_once() {
    let f = Fixture::new();
    let ctx = f.ctx();
    unsafe {
        let s = subscribe(&*f.hub, 3, cb as *const u8, ctx, drop_ctx as *const u8);
        let p = publisher(&*f.hub, 3, 12);
        let other = publisher(&*f.hub, 3, 12);
        publish_bytes(&*p, b"x");

        close(&*other);
        assert_eq!(f.ctx_of(ctx).events.lock().unwrap().len(), 1, "a non-last publisher closes silently");

        close(&*p);
        let events = f.ctx_of(ctx).events.lock().unwrap().clone();
        assert_eq!(events.len(), 2);
        assert_eq!((&events[1].0, events[1].1, &events[1].2), (&3, CLOSED_KIND, &Vec::new()));
        assert!(events[1].2.is_empty());

        // closed: publish is a no-op, pull is empty, a new publisher reaches the same subscriber
        publish_bytes(&*p, b"gone"); // no-op after close
        let mut kind = 9u8;
        assert!(get(&*f.hub, 3, &mut kind).is_empty());
        publish_bytes(&*publisher(&*f.hub, 3, 12), b"again");
        assert_eq!(f.ctx_of(ctx).events.lock().unwrap().len(), 3);

        close(&*p); // idempotent
        release_subscription(s);
    }
}

#[test]
fn scheduled_deliveries_run_only_when_the_loop_drains() {
    let f = Fixture::new();
    let ctx = f.ctx();
    let r#loop = Box::leak(Box::new(Loop { jobs: Mutex::new(Vec::new()) }));
    let loop_ptr = r#loop as *const Loop as *mut u8;
    unsafe {
        let s = subscribe_on(&*f.hub, 4, cb as *const u8, ctx, drop_ctx as *const u8, schedule as *const u8, loop_ptr);
        let inline_ctx = f.ctx();
        let inline = subscribe(&*f.hub, 4, cb as *const u8, inline_ctx, drop_ctx as *const u8);
        let p = publisher(&*f.hub, 4, 12);

        publish_bytes(&*p, b"a");
        publish_bytes(&*p, b"bb");

        // the inline subscriber ran on the publisher's thread; the scheduled one is queued
        assert_eq!(f.ctx_of(inline_ctx).events.lock().unwrap().len(), 2);
        assert!(f.ctx_of(ctx).events.lock().unwrap().is_empty());
        assert_eq!(r#loop.jobs.lock().unwrap().len(), 2);
        // pull is synchronous regardless of the scheduler
        let mut kind = 0u8;
        assert_eq!(get(&*f.hub, 4, &mut kind), b"bb");

        r#loop.drain();
        let events = f.ctx_of(ctx).events.lock().unwrap().clone();
        assert_eq!(events, vec![(4, 12, b"a".to_vec()), (4, 12, b"bb".to_vec())]);

        // Closed goes through the scheduler as well
        close(&*p);
        assert!(f.ctx_of(ctx).events.lock().unwrap().len() == 2);
        r#loop.drain();
        assert_eq!(f.ctx_of(ctx).events.lock().unwrap().last().unwrap().1, CLOSED_KIND);

        release_subscription(s);
        release_subscription(inline);
    }
}

#[test]
fn a_context_outlives_unsubscribe_while_a_job_is_pending() {
    let f = Fixture::new();
    let ctx = f.ctx();
    let r#loop = Box::leak(Box::new(Loop { jobs: Mutex::new(Vec::new()) }));
    unsafe {
        let s = subscribe_on(&*f.hub, 5, cb as *const u8, ctx, drop_ctx as *const u8, schedule as *const u8, r#loop as *const Loop as *mut u8);
        let p = publisher(&*f.hub, 5, 12);
        publish_bytes(&*p, b"pending");
        release_subscription(s);
        assert_eq!(f.drops.load(Ordering::SeqCst), 0, "the queued job still owns the context");

        // The hub already dispatched the value before the unsubscribe took effect, so the queued
        // job still carries it: the scheduler is the one that decides what a late job means.
        r#loop.drain();
        // The job was the last owner of the context: freeing it released the context. `ctx` is
        // dangling from here on and must not be read any more.
        assert_eq!(f.drops.load(Ordering::SeqCst), 1);
        drop(Box::from_raw(r#loop as *const Loop as *mut Loop));
    }
}

#[test]
fn publish_from_another_thread_reaches_the_subscriber() {
    let f = Fixture::new();
    let ctx = f.ctx();
    let done = Arc::new(AtomicBool::new(false));
    unsafe {
        let s = subscribe(&*f.hub, 6, cb as *const u8, ctx, drop_ctx as *const u8);
        let p = publisher(&*f.hub, 6, 11);
        // raw pointers are not Send: the handle is just an address, so it crosses as a usize
        let p_addr = p as usize;
        let d = done.clone();
        std::thread::spawn(move || {
            publish(&*(p_addr as *const Publisher), b"from-thread".as_ptr(), 11);
            d.store(true, Ordering::SeqCst);
        });
        while !done.load(Ordering::SeqCst) {
            std::thread::yield_now();
        }
        // Read the events while the context is still alive: releasing the subscription is what
        // frees it, and nothing may touch `ctx` afterwards.
        let events = f.ctx_of(ctx).events.lock().unwrap().clone();
        release_subscription(s);
        release_publisher(p);
        assert_eq!(events[0].2, b"from-thread");
    }
    assert_eq!(f.drops.load(Ordering::SeqCst), 1, "releasing the subscription frees the context");
}
