// Registers the Swift module with React Native's bridge (the Swift class is @objc(HubNative)).
#import <React/RCTBridgeModule.h>
#import <React/RCTEventEmitter.h>

@interface RCT_EXTERN_MODULE (HubNative, RCTEventEmitter)

RCT_EXTERN_METHOD(publishFromNative
                  : (NSNumber *)id kind
                  : (NSNumber *)kind bytes
                  : (NSArray *)bytes resolver
                  : (RCTPromiseResolveBlock)resolver rejecter
                  : (RCTPromiseRejectBlock)rejecter)

RCT_EXTERN_METHOD(closePublisher : (NSNumber *)handle)

RCT_EXTERN_METHOD(subscribeFromNative
                  : (NSNumber *)id resolver
                  : (RCTPromiseResolveBlock)resolver rejecter
                  : (RCTPromiseRejectBlock)rejecter)

RCT_EXTERN_METHOD(unsubscribeNative : (NSNumber *)handle)

RCT_EXTERN_METHOD(pullFromNative
                  : (NSNumber *)id resolver
                  : (RCTPromiseResolveBlock)resolver rejecter
                  : (RCTPromiseRejectBlock)rejecter)

RCT_EXTERN_METHOD(publishProfileFromNative
                  : (NSNumber *)id resolver
                  : (RCTPromiseResolveBlock)resolver rejecter
                  : (RCTPromiseRejectBlock)rejecter)

@end
