// No console window next to KERN when it is started from Explorer.
#![cfg_attr(windows, windows_subsystem = "windows")]

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
    // Before any thread starts: KERN launched from inside a Claude Code session
    // must not pass on its markers, or the claude tabs would not save transcripts.
    for key in kern::platform::tools::INHERITED_AGENT_MARKERS {
        // SAFETY: the process is still single-threaded here.
        unsafe { std::env::remove_var(key) };
    }
    kern::ui::run();
}
