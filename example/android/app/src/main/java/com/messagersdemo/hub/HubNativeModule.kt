package com.messagersdemo.hub

import com.facebook.react.bridge.Arguments
import com.facebook.react.bridge.Promise
import com.facebook.react.bridge.ReactApplicationContext
import com.facebook.react.bridge.ReactContextBaseJavaModule
import com.facebook.react.bridge.ReactMethod
import com.facebook.react.bridge.ReadableArray
import com.facebook.react.modules.core.DeviceEventManagerModule
import message.rs.hub.Hub
import message.rs.hub.Publisher
import message.rs.hub.Subscription
import message.rs.hub.ValueKind
import java.util.concurrent.Executors

/**
 * The Kotlin surface of the hub, exposed to JS so the demo can prove cross-language delivery on
 * Android too. Everything goes through `Hub.shared` — the same registry the Rust turbo module
 * uses, because the app links exactly one copy of the core into librustturbo.so.
 */
class HubNativeModule(private val reactContext: ReactApplicationContext) :
    ReactContextBaseJavaModule(reactContext) {

    private val publishers = HashMap<Int, Publisher>()
    private val subscriptions = HashMap<Int, Subscription>()
    private var nextHandle = 1
    private val executor = Executors.newSingleThreadExecutor { Thread(it, "hub-native") }

    override fun getName() = "HubNative"

    private fun emit(id: Long, kind: Int, bytes: ByteArray?, closed: Boolean) {
        val payload = Arguments.createMap().apply {
            putDouble("id", id.toDouble())
            putInt("kind", kind)
            putBoolean("closed", closed)
            putArray("bytes", Arguments.createArray().apply {
                bytes?.forEach { pushInt(it.toInt() and 0xFF) }
            })
        }
        reactContext
            .getJSModule(DeviceEventManagerModule.RCTDeviceEventEmitter::class.java)
            .emit("HubNativeEvent", payload)
    }

    /** Publishes from Kotlin. Returns a handle for closePublisher. */
    @ReactMethod
    fun publishFromNative(id: Double, kind: Double, bytes: ReadableArray, promise: Promise) {
        val data = ByteArray(bytes.size()) { bytes.getInt(it).toByte() }
        val publisher = Hub.shared.publisher(id.toLong(), ValueKind.fromRaw(kind.toInt()) ?: ValueKind.BYTES)
        publisher.publish(data)
        val handle = nextHandle++
        publishers[handle] = publisher
        promise.resolve(handle.toDouble())
    }

    @ReactMethod
    fun closePublisher(handle: Double) {
        publishers.remove(handle.toInt())?.close()
    }

    /** Subscribes from Kotlin; every event is forwarded to JS as HubNativeEvent. */
    @ReactMethod
    fun subscribeFromNative(id: Double, promise: Promise) {
        val sub = Hub.shared.subscribeOn(id.toLong(), executor) { eventId, kind, ptr, len ->
            if (kind == message.rs.hub.CLOSED_KIND && ptr == 0L) {
                emit(eventId, kind, null, true)
            } else {
                emit(eventId, kind, Hub.copyBytesPublic(ptr, len), false)
            }
        }
        val handle = nextHandle++
        subscriptions[handle] = sub
        promise.resolve(handle.toDouble())
    }

    @ReactMethod
    fun unsubscribeNative(handle: Double) {
        subscriptions.remove(handle.toInt())?.unsubscribe()
    }

    /** Pulls from Kotlin — the same bytes JS would pull, proving one shared store. */
    @ReactMethod
    fun pullFromNative(id: Double, promise: Promise) {
        val pulled = Hub.shared.get(id.toLong())
        if (pulled == null) {
            promise.resolve(null)
            return
        }
        promise.resolve(Arguments.createMap().apply {
            putInt("kind", pulled.kind?.raw ?: -1)
            putArray("bytes", Arguments.createArray().apply {
                pulled.bytes.forEach { pushInt(it.toInt() and 0xFF) }
            })
        })
    }

    @ReactMethod fun addListener(eventName: String) {}
    @ReactMethod fun removeListeners(count: Double) {}
}
