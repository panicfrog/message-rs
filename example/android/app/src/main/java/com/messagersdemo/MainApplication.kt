package com.messagersdemo

import android.app.Application
import com.facebook.react.PackageList
import com.facebook.react.ReactApplication
import com.facebook.react.ReactHost
import com.facebook.react.ReactNativeApplicationEntryPoint.loadReactNative
import com.facebook.react.defaults.DefaultReactHost.getDefaultReactHost

class MainApplication : Application(), ReactApplication {

  override val reactHost: ReactHost by lazy {
    getDefaultReactHost(
      context = applicationContext,
      packageList =
        PackageList(this).packages.apply {
          // The Kotlin surface of the data hub (one Rust registry shared with JS).
          add(com.messagersdemo.hub.HubNativePackage())
        },
    )
  }

  override fun onCreate() {
    super.onCreate()
    // The hub's JNI symbols live in librustturbo.so (the framework links librust_app.a, which
    // carries the core and hub-jni, into it). Must be set before message.rs.hub.Hub is touched:
    // loading a second library with its own core would give the app two registries.
    System.setProperty("hub.libName", "rustturbo")
    loadReactNative(this)
  }
}
