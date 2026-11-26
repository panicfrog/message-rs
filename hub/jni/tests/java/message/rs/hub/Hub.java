package message.rs.hub;

/** The Java twin of the Kotlin wrapper: same native declarations, same semantics. */
public class Hub implements AutoCloseable {
    static {
        System.loadLibrary("hub_jni");
    }

    public static class Companion {
        public static native long sharedHandle();
        public static native long createHandle();
        public static native void releaseHandle(long handle);
        static native long createPublisher(long handle, long id, int kind);
        static native long subscribeNative(long handle, long id, Object callback);
        static native long subscribeOnNative(long handle, long id, Object callback, Object scheduler);
        static native void unsubscribeNative(long handle);
        static native void releaseSubscription(long handle);
        static native void runJob(long jobCtx);
        static native void publishNative(long handle, byte[] payload);
        static native void closeNative(long handle);
        static native void releasePublisher(long handle);
        static native long[] getNative(long handle, long id);
        static native byte[] copyBytes(long ptr, int len);
    }

    // Kotlin's `external fun` in a companion resolves to Companion; route the same way in Java.
    static final long SHARED = Companion.sharedHandle();

    public static Hub shared() { return new Hub(SHARED, false); }
    public static Hub create() { return new Hub(Companion.createHandle(), true); }

    private final long handle;
    private final boolean owned;

    private Hub(long handle, boolean owned) {
        this.handle = handle;
        this.owned = owned;
    }

    public Publisher publisher(long id, Kind kind) {
        long h = Companion.createPublisher(handle, id, kind.raw);
        if (h == 0) throw new IllegalArgumentException("unknown kind");
        return new Publisher(h, id, kind);
    }

    public Subscription subscribe(long id, Object callback) {
        long s = Companion.subscribeNative(handle, id, callback);
        if (s == 0) throw new IllegalStateException("subscribe failed");
        return new Subscription(s, id);
    }

    public Subscription subscribeOn(long id, Object scheduler, Object callback) {
        long s = Companion.subscribeOnNative(handle, id, callback, scheduler);
        if (s == 0) throw new IllegalStateException("subscribeOn failed");
        return new Subscription(s, id);
    }

    public Pulled get(long id) {
        long[] out = Companion.getNative(handle, id);
        if (out == null) return null;
        int len = (int) out[3];
        if (len == 0) return new Pulled(null, new byte[0]);
        byte[] bytes = Companion.copyBytes(out[1], len);
        return new Pulled(null, bytes);
    }

    @Override
    public void close() {
        if (owned) Companion.releaseHandle(handle);
    }

    public enum Kind {
        BOOL(0), U8(1), I8(2), U16(3), I16(4), U32(5), I32(6),
        U64(7), I64(8), F32(9), F64(10), STRING(11), BYTES(12), MESSAGE(13);
        final int raw;
        Kind(int raw) { this.raw = raw; }
    }

    public static class Publisher implements AutoCloseable {
        private final long handle;
        final long id;
        final Kind kind;
        Publisher(long handle, long id, Kind kind) { this.handle = handle; this.id = id; this.kind = kind; }

        public void publish(byte[] payload) { Companion.publishNative(handle, payload); }
        public void close() { Companion.closeNative(handle); }
    }

    public static class Subscription implements AutoCloseable {
        private final long handle;
        final long id;
        Subscription(long handle, long id) { this.handle = handle; this.id = id; }

        public void unsubscribe() { Companion.unsubscribeNative(handle); }
        @Override public void close() { unsubscribe(); }
    }

    public static class Pulled {
        public final Kind kind;
        public final byte[] bytes;
        Pulled(Kind kind, byte[] bytes) { this.kind = kind; this.bytes = bytes; }
    }

    /** Exposed for the test driver's scheduler. */
    public static void runJobPublic(long jobCtx) { Companion.runJob(jobCtx); }
}
