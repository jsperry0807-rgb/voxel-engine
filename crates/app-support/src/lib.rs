//! Small shared helpers for the three eframe/winit binaries.
//!
//! Only presentation-independent platform policy lives here; anything that
//! needs a GPU, a window or the world belongs in the crate that owns it.

/// True when the app should force the X11/XWayland backend.
///
/// Wayland pointer capture is unreliable across compositors (the lock can
/// report success before it is active, and relative motion then stops at the
/// screen edge), so X11 is preferred whenever a display is available. Set
/// `VOXEL_VIEWER_WAYLAND=1` to force the native backend.
pub fn prefer_x11() -> bool {
    std::env::var_os("VOXEL_VIEWER_WAYLAND").is_none() && std::env::var_os("DISPLAY").is_some()
}

/// Applies [`prefer_x11`] to an eframe event loop builder.
///
/// Compiles to a no-op on platforms without an X11 backend.
pub fn prefer_x11_event_loop(options: &mut eframe::NativeOptions) {
    #[cfg(all(
        unix,
        not(any(target_os = "macos", target_os = "ios", target_os = "android"))
    ))]
    {
        use winit::platform::x11::EventLoopBuilderExtX11;

        if prefer_x11() {
            options.event_loop_builder = Some(Box::new(|builder| {
                builder.with_x11();
            }));
        }
    }
    // The parameter is unused on hosts where the block above is compiled out.
    let _ = options;
}
