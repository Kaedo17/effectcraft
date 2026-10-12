//! Android entry point for EffectCraft.
//!
//! Android's `NativeActivity` loads this library and calls [`android_main`] on a dedicated
//! thread. It opens the shared app ([`effectcraft::run`], the same code the desktop binary
//! runs) and returns when the window closes, ending the activity's main-loop thread.
//!
//! This crate exists because that export must be unmangled, and on edition 2024 the only
//! spelling (`#[unsafe(no_mangle)]`) trips the workspace-forbidden `unsafe_code` lint. The
//! crate therefore carries its own lints (see `Cargo.toml`) and holds no unsafe logic: one
//! export attribute on a safe trampoline.
//!
//! Android limitations (v1): no system file picker or message dialogs (rfd has no Android
//! backend), so Save, Open Project and the export dialogs are served in-app from app-private
//! storage while media import stays unavailable; settings, plug-ins and crash recovery use
//! app-private storage too.

/// NativeActivity's main-loop thread entry point (required; see the `android-activity` docs).
/// winit pumps the activity's lifecycle and input events inside [`effectcraft::run_android`],
/// which needs the activity's handle to create its event loop; when the activity is destroyed
/// that returns and this returns with it.
#[cfg(target_os = "android")]
#[unsafe(no_mangle)]
fn android_main(app: android_activity::AndroidApp) {
    if let Err(e) = effectcraft::run_android(app) {
        log::error!("effectcraft exited with an error: {e}");
    }
}
