//! What has to happen before the window is made.

/// iced asks wgpu for the "high performance" GPU unless `WGPU_POWER_PREF` says otherwise, which on
/// a laptop means waking the discrete one: measured on the author's machine, that costs about
/// 140 ms of start-up and 65 to 150 MB more memory for no visible gain in a viewer. The variable
/// can only be set before the process starts (setting it later needs `unsafe`, which is forbidden),
/// so Lumen starts itself again once with it set. Set `WGPU_POWER_PREF` (to `high`, say) or
/// `LUMEN_NO_REEXEC=1` to keep the default.
pub fn before_window() {
    use std::os::unix::process::CommandExt;
    if std::env::var_os("WGPU_POWER_PREF").is_some() || std::env::var_os("LUMEN_NO_REEXEC").is_some() {
        return;
    }
    let Ok(exe) = std::env::current_exe() else { return };
    // Only returns if the new process could not be started: carry on as we are.
    let error = std::process::Command::new(exe).args(std::env::args_os().skip(1)).env("WGPU_POWER_PREF", "low").exec();
    eprintln!("could not restart with the integrated GPU: {error}");
}

/// How the system knows the window.
pub fn window() -> iced::window::settings::PlatformSpecific {
    iced::window::settings::PlatformSpecific {
        application_id: "io.github.imanolgo.LumenRs".into(),
        ..Default::default()
    }
}
