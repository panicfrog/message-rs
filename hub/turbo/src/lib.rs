//! The message-rs data hub for React Native, as a Rust TurboModule.
//!
//! `#[RustTurboModule("Hub")]` is the entire spec: the method table, linkme registration and
//! the TypeScript bindings (`generated/rust-turbo/Hub.ts`) are generated from it. The module
//! wraps the process-wide `message_hub::Hub::shared()` — the same registry the Swift and
//! Kotlin surfaces talk to in a hybrid app.
//!
//! The JS-side contract (see the package README):
//! - `subscribe(id, cb)` returns a subscription id; `cb` is invoked on the JS thread with a
//!   `HubEvent` object (`{ kind, bytes }`, `kind == 255 && bytes == null` means Closed).
//! - Push is a wake-up: the event carries a small snapshot; for payload bytes call `get`.
//! - `get(id)` returns the newest payload zero-copy as an ArrayBuffer (`Pulled`), or null
//!   when nothing is published.

use jsi::{JsiArrayBuffer, RuntimeHandle};
use jsi_turbo::{RustTurboModule, TurboCallback, TurboStruct};
use message_hub::{Event, Hub as CoreHub, Subscription, ValueKind};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

/// The event object handed to a JS callback. `bytes` carries the payload for small values;
/// large payloads are pulled through `get` (the callback is a wake-up).
#[derive(Serialize, Deserialize, TurboStruct)]
pub struct HubEvent {
    pub kind: f64,
    /// A copy of the payload (the JS GC owns it); null for Closed.
    pub bytes: Option<Vec<u8>>,
    /// True when this is the Closed event (last publisher of the id went away).
    pub closed: bool,
}

#[derive(Default)]
pub struct Hub;

#[RustTurboModule("Hub")]
impl Hub {
    /// Subscribes `cb` to `id`; returns the subscription id (pass it to `unsubscribe`).
    /// Every publish of `id` invokes `cb` on the JS thread with a `HubEvent`.
    pub fn subscribe(&self, id: f64, cb: TurboCallback) -> anyhow::Result<f64> {
        let core = CoreHub::shared();
        let sub_id = NEXT_SUB.fetch_add(1, Ordering::Relaxed) as f64 + 1.0;
        let wrapper = Arc::new(SubscriptionWrapper::default_wrapper());
        let subscription = core.subscribe(id as u32, {
            let cb = cb.clone();
            Arc::new(move |_id, event: Event<'_>| {
                let ev = match event {
                    Event::Value(kind, bytes) => HubEvent {
                        kind: kind as u8 as f64,
                        bytes: Some(bytes.to_vec()),
                        closed: false,
                    },
                    Event::Closed => HubEvent { kind: 255.0, bytes: None, closed: true },
                };
                cb.call(ev);
            })
        });
        wrapper.store(subscription);
        subs().lock().unwrap().insert(sub_id as u64, (wrapper, cb));
        Ok(sub_id)
    }

    /// Stops a subscription (idempotent; unknown ids are a no-op).
    pub fn unsubscribe(&self, sub_id: f64) -> anyhow::Result<()> {
        if let Some((wrapper, _cb)) = subs().lock().unwrap().remove(&(sub_id as u64)) {
            wrapper.unsubscribe();
        }
        Ok(())
    }

    /// Publishes a payload through the publisher created by `create_publisher`. `data` is
    /// read in place (no copy on the way in). Unknown handles are an error: the JS side must
    /// create a publisher first (one-shot publishers would close on drop and deliver Closed).
    pub fn publish<'rt>(
        &self,
        _rt: &mut RuntimeHandle<'rt>,
        handle: f64,
        data: JsiArrayBuffer<'rt>,
    ) -> anyhow::Result<()> {
        let p = pubs()
            .lock()
            .unwrap()
            .get(&(handle as u64))
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("unknown publisher handle {handle}"))?;
        p.publish(data.data(_rt));
        Ok(())
    }

    /// Pulls the newest payload of `id` as an ArrayBuffer (zero-copy into JS; an empty
    /// buffer means nothing is published). The kind comes from `getKind` right after.
    pub fn get<'rt>(&self, rt: &mut RuntimeHandle<'rt>, id: f64) -> anyhow::Result<JsiArrayBuffer<'rt>> {
        match CoreHub::shared().get(id as u32) {
            Some((kind, bytes)) => {
                LAST_KIND.with(|c| c.set(kind as u8 as f64));
                // Arc<[u8]> -> Vec: one copy to hand the allocation to the JS GC.
                let v: Vec<u8> = bytes.to_vec();
                Ok(JsiArrayBuffer::from_vec(v, rt))
            }
            None => {
                LAST_KIND.with(|c| c.set(-1.0));
                Ok(JsiArrayBuffer::from_vec(Vec::new(), rt))
            }
        }
    }

    /// The kind of the value `get` just returned (-1 when nothing is published). The JS side
    /// reads it before calling `get` again for the same id.
    #[turbo(getter)]
    pub fn get_kind(&self) -> anyhow::Result<f64> {
        Ok(LAST_KIND.with(|c| c.get()))
    }

    /// Creates a publisher for `(id, kind)`; returns its handle.
    pub fn create_publisher(&self, id: f64, kind: f64) -> anyhow::Result<f64> {
        let core = CoreHub::shared();
        let kind = ValueKind::from_u8(kind as u8)
            .ok_or_else(|| anyhow::anyhow!("unknown kind {}", kind as u8))?;
        let handle = NEXT_PUB.fetch_add(1, Ordering::Relaxed) as f64 + 1.0;
        let publisher = Arc::new(core.publisher(id as u32, kind));
        pubs().lock().unwrap().insert(handle as u64, publisher);
        Ok(handle)
    }

    /// Closes a publisher: subscribers get Closed when this is the id's last live one.
    pub fn close_publisher(&self, handle: f64) -> anyhow::Result<()> {
        if let Some(p) = pubs().lock().unwrap().remove(&(handle as u64)) {
            p.close();
        }
        Ok(())
    }
}

// Keep the platform's native surface alive in the aggregate library: nothing in Rust calls
// those entry points (Swift/C++ and the JVM do), so without an explicit reference the linker
// drops their objects and the other ends find no symbols. See example/PLAN.md.
#[cfg(any(target_os = "ios", target_os = "macos"))]
#[used]
static KEEP_NATIVE_SURFACE: extern "C" fn() = hub_ffi::message_hub_ffi_force_link;

#[cfg(target_os = "android")]
#[used]
static KEEP_NATIVE_SURFACE: extern "C" fn() = hub_jni::message_hub_jni_force_link;

// `get` returns only the buffer; the kind travels through this cell (read by `getKind`).
thread_local! {
    static LAST_KIND: std::cell::Cell<f64> = const { std::cell::Cell::new(-1.0) };
}

// ---- handle registries (plain ids, like the JNI surface) ----

static NEXT_SUB: AtomicU64 = AtomicU64::new(0);
static NEXT_PUB: AtomicU64 = AtomicU64::new(0);
static SUBS: OnceLock<Mutex<HashMap<u64, (Arc<SubscriptionWrapper>, TurboCallback)>>> = OnceLock::new();
static PUBS: OnceLock<Mutex<HashMap<u64, Arc<message_hub::Publisher>>>> = OnceLock::new();

fn subs() -> &'static Mutex<HashMap<u64, (Arc<SubscriptionWrapper>, TurboCallback)>> {
    SUBS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn pubs() -> &'static Mutex<HashMap<u64, Arc<message_hub::Publisher>>> {
    PUBS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// A shareable RAII wrapper: `store` owns the subscription, `unsubscribe` is idempotent and
/// works from any thread (the module and the JS side both hold Arcs).
struct SubscriptionWrapper {
    inner: Mutex<Option<Subscription>>,
}

impl SubscriptionWrapper {
    fn default_wrapper() -> SubscriptionWrapper {
        SubscriptionWrapper { inner: Mutex::new(None) }
    }

    fn store(&self, sub: Subscription) {
        *self.inner.lock().unwrap() = Some(sub);
    }

    fn unsubscribe(&self) {
        if let Some(s) = self.inner.lock().unwrap().take() {
            s.unsubscribe();
        }
    }
}

impl Drop for SubscriptionWrapper {
    fn drop(&mut self) {
        self.unsubscribe();
    }
}

// The wrapper is only touched behind its Mutex from any thread.
unsafe impl Send for SubscriptionWrapper {}
unsafe impl Sync for SubscriptionWrapper {}


