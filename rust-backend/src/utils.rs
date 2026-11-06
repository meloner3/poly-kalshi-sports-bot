//! Utility functions

use std::path::PathBuf;
use std::sync::OnceLock;

/// Global debug log path
static DEBUG_LOG_PATH: OnceLock<PathBuf> = OnceLock::new();

/// Initialize debug log path (should be called once at startup)
pub fn init_debug_log_path(workspace_root: Option<&str>) {
    let path = if let Some(root) = workspace_root {
        PathBuf::from(root).join("logs").join("debug.log")
    } else {
        // Fallback: try to find workspace root from current exe location
        std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(|p| p.to_path_buf()))
            .map(|mut p| {
                // If we're in target/debug or target/release, go up to workspace root
                if p.ends_with("debug") || p.ends_with("release") {
                    p.pop(); // remove debug/release
                    p.pop(); // remove target
                }
                p.join("logs").join("debug.log")
            })
            .unwrap_or_else(|| PathBuf::from("logs/debug.log"))
    };

    // Ensure logs directory exists
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    let _ = DEBUG_LOG_PATH.set(path);
}

/// Get debug log path
pub fn get_debug_log_path() -> PathBuf {
    DEBUG_LOG_PATH.get().cloned().unwrap_or_else(|| {
        // Fallback if not initialized
        let path = PathBuf::from("logs/debug.log");
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        path
    })
}
