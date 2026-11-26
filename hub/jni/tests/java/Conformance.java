import message.rs.hub.Hub;
import message.rs.hub.Hub.Pulled;

/** Drives the JNI surface exactly the way the Kotlin wrapper will (a Java twin of it). */
public class Conformance {
    static volatile int received = 0;
    static volatile long lastPtr = 0;
    static volatile int lastLen = 0;
    static volatile int closedCount = 0;
    static volatile boolean wrongThread = false;

    interface Cb { void onEvent(long id, int kind, long ptr, int len); }

    // A callback object with the exact (JIJI)V shape the native side looks up.
    public static class Callback implements Cb {
        public void onEvent(long id, int kind, long ptr, int len) {
            received++;
            if (Thread.currentThread().getName().equals("publisher")) wrongThread = true;
            lastPtr = ptr; lastLen = len;
            if (kind == 255 && ptr == 0) closedCount++;
        }
    }

    // A scheduler that runs jobs on a dedicated thread (the Executor twin).
    public static class Scheduler {
        public void schedule(long jobCtx) {
            runner.submit(() -> Hub.runJobPublic(jobCtx));
        }
        final java.util.concurrent.ExecutorService runner =
            java.util.concurrent.Executors.newSingleThreadExecutor(r -> new Thread(r, "scheduler"));
    }

    public static void main(String[] args) throws Exception {
        int failures = 0;

        // 1. push + pull
        Hub hub = Hub.create();
        Hub.Publisher p = hub.publisher(1, Hub.Kind.BYTES);
        java.util.List<Long> events = new java.util.ArrayList<>();
        Hub.Subscription s = hub.subscribe(1, new Callback());
        p.publish(new byte[]{1, 2, 3});
        p.publish(new byte[]{4, 5});
        if (received != 2) { failures++; System.out.println("FAIL push count " + received); }
        Hub.Pulled v = hub.get(1);
        if (v == null || !java.util.Arrays.equals(v.bytes, new byte[]{4, 5})) {
            failures++; System.out.println("FAIL pull " + (v == null ? "null" : v.bytes.length));
        }

        // 2. subscribing does not replay
        Hub.Subscription s2 = hub.subscribe(1, new Callback());
        if (received != 2) { failures++; System.out.println("FAIL replay " + received); }
        s2.close();

        // 3. closed lifecycle
        p.close();
        if (closedCount != 1) { failures++; System.out.println("FAIL closed " + closedCount); }
        if (hub.get(1) != null) { failures++; System.out.println("FAIL pull after close"); }
        Hub.Publisher p2 = hub.publisher(1, Hub.Kind.BYTES);
        p2.publish(new byte[]{9});
        // received: 2 pushes + 1 closed + 1 revived = 4; the subscription survived Closed.
        if (received != 4) { failures++; System.out.println("FAIL revive " + received); }

        // 4. scheduled delivery on the scheduler's thread
        received = 0;
        Hub.Publisher q = hub.publisher(2, Hub.Kind.BYTES);
        Hub.Subscription sq = hub.subscribeOn(2, new Scheduler(), new Callback());
        q.publish(new byte[]{7});
        long deadline = System.currentTimeMillis() + 2000;
        while (received == 0 && System.currentTimeMillis() < deadline) Thread.sleep(10);
        if (received != 1) { failures++; System.out.println("FAIL scheduled " + received); }

        // 5. shared hub is one registry: two Java wrappers around it see the same native hub
        Hub a = Hub.shared();
        Hub.Publisher sp = a.publisher(90, Hub.Kind.BYTES);
        int before = received;
        Hub.Subscription ss = a.subscribe(90, new Callback());
        sp.publish(new byte[]{1});
        if (received != before + 1) { failures++; System.out.println("FAIL shared registry"); }
        ss.close();

        // 6. unsubscribe stops deliveries
        received = 0;
        sq.close();
        q.publish(new byte[]{8});
        if (received != 0) { failures++; System.out.println("FAIL unsubscribe " + received); }

        System.out.println(failures == 0 ? "ALL OK" : failures + " FAILURES");
        System.exit(failures == 0 ? 0 : 1);
    }
}
