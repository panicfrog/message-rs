require 'json'

# The Swift surface of the data hub inside the demo app, in "superlib mode": the wrapper
# sources compile against the cxxbridge header only — the Rust symbols come from the
# librust-app.a that the RustTurbo pod already links (hub-turbo carries hub-ffi). Linking a
# second copy of the core here would give the app two Hub::shared() registries.
Pod::Spec.new do |s|
  s.name         = "MessageHubNative"
  s.version      = "0.1.0"
  s.summary      = "message-rs data hub: the Swift surface, symbols from the app's Rust superlib"
  s.license      = { :type => "MIT" }
  s.authors      = { "message-rs" => "https://github.com/panicfrog/message-rs" }
  s.homepage     = "https://github.com/panicfrog/message-rs"
  s.source       = { :path => "." }
  s.platform     = :ios, "15.1"

  # the wrapper sources are the package's own files (no copy to drift)
  s.source_files = "wrapper/*.swift", "*.{swift,h,m,mm}"
  # the wrapper does `import MessageHubCxx`; this module map gives that name to the header
  s.preserve_paths = "module.modulemap"

  s.pod_target_xcconfig = {
    # the cxxbridge header + the cxx runtime header, as the MessageHubCxx Clang target exposes them
    "HEADER_SEARCH_PATHS" => "\"$(PODS_TARGET_SRCROOT)/../../../hub/swift/Sources/MessageHubCxx/include\"",
    "SWIFT_INCLUDE_PATHS" => "\"$(PODS_TARGET_SRCROOT)\"",
    # the wrapper uses C++ interop; RN 0.88 headers need C++20 (concepts), so match that
    "SWIFT_VERSION" => "5.9",
    "OTHER_SWIFT_FLAGS" => "-cxx-interoperability-mode=default -Xcc -std=c++20",
    "CLANG_CXX_LANGUAGE_STANDARD" => "c++20",
    "DEFINES_MODULE" => "YES",
  }

  install_modules_dependencies(s)
end
