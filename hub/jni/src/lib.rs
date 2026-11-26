//! `hub-jni`: the JNI surface of `message-hub` for Kotlin (Android).
//!
//! Kotlin sees `external fun` declarations on the `Hub.Companion` object (the only place
//! native methods are declared, so the JVM symbol names are stable). The shapes mirror the
//! core's semantics (hub/DESIGN.md section 3) and the Swift bridge's conventions: handles
//! are raw pointers held by the Kotlin wrapper (`Long`), payload copies cross as byte
//! arrays, and `Closed` is delivered as `ptr == 0`.
//!
//! Java callbacks are objects: the Rust side keeps a `GlobalRef` with a pre-looked-up
//! `onEvent` method id; a scheduler is a `GlobalRef` with a `schedule(jobCtx: Long)` method
//! that must run `runJob(jobCtx)` exactly once per job.

use jni::objects::{GlobalRef, JClass, JMethodID, JObject, JValue};
use jni::signature::{Primitive, ReturnType};
use jni::sys::{jbyte, jbyteArray, jint, jlong, jlongArray};
use jni::{JNIEnv, JavaVM};
use message_hub::{Callback, Event, Hub as CoreHub, Publisher as CorePublisher, Subscription as CoreSubscription};
use std::ptr;
use std::sync::Arc;

struct SubState {
    vm: JavaVM,
    /// The Kotlin callback: `fun onEvent(id: Long, kind: Int, ptr: Long, len: Int)`.
    callback: GlobalRef,
    on_event: JMethodID,
    /// Optional scheduler: `fun schedule(jobCtx: Long)`.
    schedule: Option<(GlobalRef, JMethodID)>,
}

// The foreign references are only ever used through JNI calls, from any thread.
unsafe impl Send for SubState {}
unsafe impl Sync for SubState {}

impl SubState {
    fn deliver(&self, id: u32, kind_u8: u8, bytes: Option<&[u8]>) {
        let Ok(mut env) = self.vm.attach_current_thread() else { return };
        let (ptr_v, len) = match bytes {
            Some(b) => (b.as_ptr() as jlong, b.len() as jint),
            None => (0, 0), // Closed: a null payload
        };
        // SAFETY: the method id was looked up at subscribe time on the callback's class, the
        // object is alive (we hold the global reference until the subscription is dropped)
        // and the arguments match the (JIJI)V signature.
        unsafe {
            let _ = env.call_method_unchecked(
                JObject::from_raw(self.callback.as_raw()),
                self.on_event,
                ReturnType::Primitive(Primitive::Void),
                &[
                    JValue::Long(id as jlong).as_jni(),
                    JValue::Int(kind_u8 as jint).as_jni(),
                    JValue::Long(ptr_v).as_jni(),
                    JValue::Int(len).as_jni(),
                ],
            );
        }
    }

    fn schedule_job(&self, job: Job) {
        let job_ctx = Box::into_raw(Box::new(job)) as jlong;
        let Ok(mut env) = self.vm.attach_current_thread() else { return };
        let Some((sched, method)) = self.schedule.as_ref() else { return };
        // SAFETY: same contract as `deliver`.
        unsafe {
            let _ = env.call_method_unchecked(
                JObject::from_raw(sched.as_raw()),
                *method,
                ReturnType::Primitive(Primitive::Void),
                &[JValue::Long(job_ctx).as_jni()],
            );
        }
    }
}

/// One scheduled delivery, owned by the scheduler until it calls `runJob`.
struct Job {
    sub: Arc<SubState>,
    id: u32,
    kind_u8: u8,
    bytes: Option<Arc<[u8]>>,
}

/// The kind a `Closed` event carries; outside `ValueKind`'s range on purpose.
pub const CLOSED_KIND: u8 = 255;

fn make_callback(sub: Arc<SubState>) -> Callback {
    let scheduled = sub.schedule.is_some();
    Arc::new(move |id, event: Event<'_>| {
        let (kind_u8, bytes) = match event {
            Event::Value(kind, b) => (kind as u8, Some(Arc::from(b))),
            Event::Closed => (CLOSED_KIND, None),
        };
        if !scheduled {
            sub.deliver(id, kind_u8, bytes.as_deref());
            return;
        }
        let job = Job { sub: Arc::clone(&sub), id, kind_u8, bytes };
        sub.schedule_job(job);
    })
}

fn throw(env: &mut JNIEnv, class: &str, msg: &str) {
    let _ = env.throw_new(class, msg);
}

unsafe fn deref_hub<'a>(env: &mut JNIEnv<'a>, handle: jlong) -> Option<&'a CoreHub> {
    if handle == 0 {
        throw(env, "java/lang/IllegalArgumentException", "hub handle is null");
        return None;
    }
    Some(unsafe { &*(handle as *const CoreHub) })
}

// ---- Hub.Companion entry points (the only symbols Kotlin declares native) ----

/// The process-wide registry, as a stable pointer (never freed).
#[unsafe(no_mangle)]
pub extern "system" fn Java_message_rs_hub_Hub_00024Companion_sharedHandle(
    _env: JNIEnv,
    _class: JClass,
) -> jlong {
    ptr::from_ref(CoreHub::shared()) as jlong
}

/// An isolated registry for tests; freed by `releaseHandle`.
#[unsafe(no_mangle)]
pub extern "system" fn Java_message_rs_hub_Hub_00024Companion_createHandle(
    _env: JNIEnv,
    _class: JClass,
) -> jlong {
    Box::into_raw(Box::new(CoreHub::new())) as jlong
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_message_rs_hub_Hub_00024Companion_releaseHandle(
    _env: JNIEnv,
    _class: JClass,
    handle: jlong,
) {
    if handle != 0 {
        drop(unsafe { Box::from_raw(handle as *mut CoreHub) });
    }
}

/// Returns 0 (with an exception) when `kind_u8` is not a valid kind.
#[unsafe(no_mangle)]
pub extern "system" fn Java_message_rs_hub_Hub_00024Companion_createPublisher(
    mut env: JNIEnv,
    _class: JClass,
    hub_handle: jlong,
    id: jint,
    kind_u8: jbyte,
) -> jlong {
    let Some(h) = (unsafe { deref_hub(&mut env, hub_handle) }) else { return 0 };
    match message_hub::ValueKind::from_u8(kind_u8 as u8) {
        Some(kind) => Box::into_raw(Box::new(h.publisher(id as u32, kind))) as jlong,
        None => {
            throw(&mut env, "java/lang/IllegalArgumentException", "unknown kind");
            0
        }
    }
}

/// Publishes one payload; a no-op after close. Copies the JVM bytes once.
#[unsafe(no_mangle)]
pub extern "system" fn Java_message_rs_hub_Hub_00024Companion_publishNative(
    mut env: JNIEnv,
    _class: JClass,
    handle: jlong,
    payload: jbyteArray,
) {
    if handle == 0 {
        throw(&mut env, "java/lang/IllegalArgumentException", "publisher handle is null");
        return;
    }
    let p = unsafe { &*(handle as *const CorePublisher) };
    let bytes: Vec<u8> = if payload.is_null() {
        Vec::new()
    } else {
        env.convert_byte_array(unsafe { jni::objects::JByteArray::from_raw(payload) })
            .map(|v| v.into())
            .unwrap_or_default()
    };
    p.publish(&bytes);
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_message_rs_hub_Hub_00024Companion_closeNative(
    _env: JNIEnv,
    _class: JClass,
    handle: jlong,
) {
    if handle != 0 {
        unsafe { (*(handle as *const CorePublisher)).close() };
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_message_rs_hub_Hub_00024Companion_releasePublisher(
    _env: JNIEnv,
    _class: JClass,
    handle: jlong,
) {
    if handle != 0 {
        drop(unsafe { Box::from_raw(handle as *mut CorePublisher) }); // Drop closes
    }
}

/// Inline subscription: deliveries run on the publisher's thread.
#[unsafe(no_mangle)]
pub extern "system" fn Java_message_rs_hub_Hub_00024Companion_subscribeNative(
    mut env: JNIEnv,
    _class: JClass,
    hub_handle: jlong,
    id: jint,
    callback: JObject,
) -> jlong {
    subscribe_impl(&mut env, hub_handle, id, callback, None)
}

/// Scheduled subscription: `scheduler.schedule(jobCtx)` must run `runJob(jobCtx)` once.
#[unsafe(no_mangle)]
pub extern "system" fn Java_message_rs_hub_Hub_00024Companion_subscribeOnNative(
    mut env: JNIEnv,
    _class: JClass,
    hub_handle: jlong,
    id: jint,
    callback: JObject,
    scheduler: JObject,
) -> jlong {
    subscribe_impl(&mut env, hub_handle, id, callback, Some(scheduler))
}

fn subscribe_impl(
    env: &mut JNIEnv,
    hub_handle: jlong,
    id: jint,
    callback: JObject,
    scheduler: Option<JObject>,
) -> jlong {
    let Some(h) = (unsafe { deref_hub(env, hub_handle) }) else { return 0 };
    let vm = match env.get_java_vm() {
        Ok(v) => v,
        Err(_) => return 0,
    };
    // Look up `onEvent` once on the callback's class; keep the object alive globally.
    // Re-borrow the same local reference: get_object_class consumes the JObject by value.
    let cb_class = match env.get_object_class(unsafe { JObject::from_raw(callback.as_raw()) }) {
        Ok(c) => c,
        Err(_) => return 0,
    };
    let on_event: JMethodID = match env.get_method_id(cb_class, "onEvent", "(JIJI)V") {
        Ok(m) => m,
        Err(_) => {
            throw(env, "java/lang/IllegalArgumentException", "callback must declare onEvent(id: Long, kind: Int, ptr: Long, len: Int)");
            return 0;
        }
    };
    let callback_ref = match env.new_global_ref(callback) {
        Ok(r) => r,
        Err(_) => return 0,
    };
    let schedule = match scheduler {
        Some(s) => {
            let s_class = match env.get_object_class(unsafe { JObject::from_raw(s.as_raw()) }) {
                Ok(c) => c,
                Err(_) => return 0,
            };
            match (env.get_method_id(s_class, "schedule", "(J)V"), env.new_global_ref(s)) {
                (Ok(m), Ok(r)) => Some((r, m)),
                _ => {
                    throw(env, "java/lang/IllegalArgumentException", "scheduler must declare schedule(jobCtx: Long)");
                    return 0;
                }
            }
        }
        None => None,
    };
    let sub = Arc::new(SubState { vm, callback: callback_ref, on_event, schedule });
    let callback = make_callback(Arc::clone(&sub));
    let subscription = h.subscribe(id as u32, callback);
    Box::into_raw(Box::new(subscription)) as jlong
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_message_rs_hub_Hub_00024Companion_unsubscribeNative(
    _env: JNIEnv,
    _class: JClass,
    handle: jlong,
) {
    if handle != 0 {
        unsafe { (*(handle as *const CoreSubscription)).unsubscribe() };
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_message_rs_hub_Hub_00024Companion_releaseSubscription(
    _env: JNIEnv,
    _class: JClass,
    handle: jlong,
) {
    if handle != 0 {
        drop(unsafe { Box::from_raw(handle as *mut CoreSubscription) }); // Drop unsubscribes
    }
}

/// Runs one scheduled delivery and frees the job. Exactly once per job ctx.
#[unsafe(no_mangle)]
pub extern "system" fn Java_message_rs_hub_Hub_00024Companion_runJob(
    _env: JNIEnv,
    _class: JClass,
    job_ctx: jlong,
) {
    if job_ctx == 0 {
        return;
    }
    unsafe {
        let job = Box::from_raw(job_ctx as *mut Job);
        job.sub.deliver(job.id, job.kind_u8, job.bytes.as_deref());
    }
}

/// Pulls the newest value of `id`: null when nothing is published, otherwise
/// `[kind, ptr, end, len]` describing the stored buffer.
#[unsafe(no_mangle)]
pub extern "system" fn Java_message_rs_hub_Hub_00024Companion_getNative(
    mut env: JNIEnv,
    _class: JClass,
    hub_handle: jlong,
    id: jint,
) -> jlongArray {
    let Some(h) = (unsafe { deref_hub(&mut env, hub_handle) }) else { return ptr::null_mut() };
    let Some((kind, bytes)) = h.get(id as u32) else { return ptr::null_mut() };
    let out: [jlong; 4] = [
        kind as u8 as jlong,
        bytes.as_ptr() as jlong,
        (bytes.as_ptr() as usize + bytes.len()) as jlong,
        bytes.len() as jlong,
    ];
    match env.new_long_array(out.len() as i32) {
        Ok(a) => {
            let _ = env.set_long_array_region(&a, 0, &out);
            a.as_raw()
        }
        Err(_) => ptr::null_mut(),
    }
}

/// Reads `len` bytes at `ptr` into a fresh `ByteArray`. One copy on the pull path (the stored
/// payload lives until the next publish of the id).
#[unsafe(no_mangle)]
pub extern "system" fn Java_message_rs_hub_Hub_00024Companion_copyBytes(
    env: JNIEnv,
    _class: JClass,
    ptr_v: jlong,
    len: jint,
) -> jbyteArray {
    if ptr_v == 0 || len <= 0 {
        return ptr::null_mut();
    }
    // SAFETY: the pair (ptr, len) came from `getNative` of the same id, unread since.
    let src = unsafe { std::slice::from_raw_parts(ptr_v as *const u8, len as usize) };
    match env.byte_array_from_slice(src) {
        Ok(a) => a.as_raw(),
        Err(_) => ptr::null_mut(),
    }
}

// ---- Aliases for Kotlin's @JvmStatic external declarations ----
// Kotlin compiles `companion object { @JvmStatic external fun f }` to a static method on the
// outer class, so the JVM looks up `Java_..._Hub_f` — export both spellings for every entry.

#[unsafe(no_mangle)]
pub extern "system" fn Java_message_rs_hub_Hub_sharedHandle(env: JNIEnv, c: JClass) -> jlong {
    Java_message_rs_hub_Hub_00024Companion_sharedHandle(env, c)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_message_rs_hub_Hub_createHandle(env: JNIEnv, c: JClass) -> jlong {
    Java_message_rs_hub_Hub_00024Companion_createHandle(env, c)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_message_rs_hub_Hub_releaseHandle(env: JNIEnv, c: JClass, h: jlong) {
    Java_message_rs_hub_Hub_00024Companion_releaseHandle(env, c, h)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_message_rs_hub_Hub_createPublisher(env: JNIEnv, c: JClass, h: jlong, id: jint, k: jbyte) -> jlong {
    Java_message_rs_hub_Hub_00024Companion_createPublisher(env, c, h, id, k)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_message_rs_hub_Hub_subscribeNative(env: JNIEnv, c: JClass, h: jlong, id: jint, cb: JObject) -> jlong {
    Java_message_rs_hub_Hub_00024Companion_subscribeNative(env, c, h, id, cb)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_message_rs_hub_Hub_subscribeOnNative(env: JNIEnv, c: JClass, h: jlong, id: jint, cb: JObject, s: JObject) -> jlong {
    Java_message_rs_hub_Hub_00024Companion_subscribeOnNative(env, c, h, id, cb, s)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_message_rs_hub_Hub_unsubscribeNative(env: JNIEnv, c: JClass, h: jlong) {
    Java_message_rs_hub_Hub_00024Companion_unsubscribeNative(env, c, h)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_message_rs_hub_Hub_releaseSubscription(env: JNIEnv, c: JClass, h: jlong) {
    Java_message_rs_hub_Hub_00024Companion_releaseSubscription(env, c, h)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_message_rs_hub_Hub_runJob(env: JNIEnv, c: JClass, j: jlong) {
    Java_message_rs_hub_Hub_00024Companion_runJob(env, c, j)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_message_rs_hub_Hub_publishNative(env: JNIEnv, c: JClass, h: jlong, p: jbyteArray) {
    Java_message_rs_hub_Hub_00024Companion_publishNative(env, c, h, p)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_message_rs_hub_Hub_closeNative(env: JNIEnv, c: JClass, h: jlong) {
    Java_message_rs_hub_Hub_00024Companion_closeNative(env, c, h)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_message_rs_hub_Hub_releasePublisher(env: JNIEnv, c: JClass, h: jlong) {
    Java_message_rs_hub_Hub_00024Companion_releasePublisher(env, c, h)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_message_rs_hub_Hub_getNative(env: JNIEnv, c: JClass, h: jlong, id: jint) -> jlongArray {
    Java_message_rs_hub_Hub_00024Companion_getNative(env, c, h, id)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_message_rs_hub_Hub_copyBytes(env: JNIEnv, c: JClass, p: jlong, l: jint) -> jbyteArray {
    Java_message_rs_hub_Hub_00024Companion_copyBytes(env, c, p, l)
}

/// Force-link anchor: the JNI entry points are only called from the JVM, so an aggregate
/// library needs an explicit reference to keep this crate's objects. See example/PLAN.md.
#[unsafe(no_mangle)]
pub extern "C" fn message_hub_jni_force_link() {}
