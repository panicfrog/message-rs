import Foundation
import React

/// The Swift surface of the hub, exposed to JS so the demo can prove cross-language delivery:
/// JS publishes and this subscription fires; this publishes and the JS channel receives.
///
/// Everything goes through `Hub.shared` — the same registry the Rust turbo module uses, because
/// the app links exactly one copy of the core (see example/PLAN.md).
@objc(HubNative)
public class HubNative: RCTEventEmitter {
  private var publishers: [NSNumber: Publisher] = [:]
  private var subscriptions: [NSNumber: Subscription] = [:]
  private var nextHandle: UInt32 = 1
  private var listening = false

  public override init() {
    super.init()
  }

  @objc public override static func requiresMainQueueSetup() -> Bool { false }

  public override func supportedEvents() -> [String]! { ["HubNativeEvent"] }

  public override func startObserving() { listening = true }
  public override func stopObserving() { listening = false }

  /// Publishes from Swift. Returns a handle for `closePublisher`.
  @objc(publishFromNative:kind:bytes:resolver:rejecter:)
  func publishFromNative(
    id: NSNumber, kind: NSNumber, bytes: [UInt8],
    resolver: @escaping RCTPromiseResolveBlock, rejecter: @escaping RCTPromiseRejectBlock
  ) {
    let publisher = Publisher(hub: .shared, id: id.uint32Value, kind: ValueKind(rawValue: kind.uint8Value) ?? .bytes)
    publisher.publish(bytes)
    let handle = NSNumber(value: nextHandle)
    nextHandle += 1
    publishers[handle] = publisher
    resolver(handle.doubleValue)
  }

  /// Closes a native publisher: JS subscribers get Closed when it was the id's last one.
  @objc(closePublisher:)
  func closePublisher(handle: NSNumber) {
    if let p = publishers.removeValue(forKey: handle) {
      p.close()
    }
  }

  /// Subscribes from Swift; every event is forwarded to JS as `HubNativeEvent`.
  @objc(subscribeFromNative:resolver:rejecter:)
  func subscribeFromNative(
    id: NSNumber,
    resolver: @escaping RCTPromiseResolveBlock, rejecter: @escaping RCTPromiseRejectBlock
  ) {
    let sub = Hub.shared.subscribe(id: id.uint32Value) { [weak self] eventId, kind, ptr, len in
      guard let self, self.listening else { return }
      let bytes: [UInt8] = ptr.map { Array(UnsafeBufferPointer(start: $0, count: len)) } ?? []
      self.sendEvent(withName: "HubNativeEvent", body: [
        "id": eventId,
        "kind": kind,
        "bytes": bytes,
        "closed": kind == 255 && ptr == nil,
      ])
    }
    let handle = NSNumber(value: nextHandle)
    nextHandle += 1
    subscriptions[handle] = sub
    resolver(handle.doubleValue)
  }

  @objc(unsubscribeNative:)
  func unsubscribeNative(handle: NSNumber) {
    if let s = subscriptions.removeValue(forKey: handle) {
      s.unsubscribe()
    }
  }

  /// Pulls from Swift — the same bytes JS would pull, proving one shared store.
  @objc(pullFromNative:resolver:rejecter:)
  func pullFromNative(
    id: NSNumber,
    resolver: @escaping RCTPromiseResolveBlock, rejecter: @escaping RCTPromiseRejectBlock
  ) {
    if let (kind, bytes) = Hub.shared.get(id: id.uint32Value) {
      resolver(["kind": kind.rawValue, "bytes": bytes])
    } else {
      resolver(nil)
    }
  }

  /// Publishes a real generated struct (`Profile`) through the unified protocol — no manual
  /// encode step, exactly like `publish(id:, UInt16(1))`. Reports the raw bytes back so JS can
  /// decode them with its own `Profile.codec` and check both ends agree (see scenarios.ts).
  @objc(publishProfileFromNative:resolver:rejecter:)
  func publishProfileFromNative(
    id: NSNumber,
    resolver: @escaping RCTPromiseResolveBlock, rejecter: @escaping RCTPromiseRejectBlock
  ) {
    let profile = Profile(age: 30, bio: "hello", nickname: "yamada", verified: true)
    Hub.shared.publish(id: id.uint32Value, profile)
    resolver(Array(profile.encode()))
  }
}
