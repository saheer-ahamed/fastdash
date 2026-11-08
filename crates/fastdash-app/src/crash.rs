//! The last line of defence: a panic anywhere shows a plain, reassuring
//! message instead of the window vanishing, and leaves a report behind.
//!
//! Everyday users see only the reassurance. The panic message, its location
//! and a backtrace are shown in developer mode, and always written to
//! `crash.log` next to the config, where a bug report can pick them up.

use std::backtrace::Backtrace;
use std::io::Write;
use std::panic::PanicHookInfo;
use std::sync::atomic::{AtomicBool, Ordering};

use directories::ProjectDirs;

use crate::i18n::t;
use crate::platform;

/// Whether developer mode is on, mirrored here so the hook never has to reach
/// into GPUI state from a panicking thread.
static DEV_MODE: AtomicBool = AtomicBool::new(false);

pub fn set_dev_mode(on: bool) {
    DEV_MODE.store(on, Ordering::Relaxed);
}

pub fn install() {
    std::panic::set_hook(Box::new(|info| {
        let details = details(info);
        eprintln!("{details}");
        write_report(&details);
        let mut body = format!("{}\n\n{}", t("error.crashLede"), t("error.crashHint"));
        if DEV_MODE.load(Ordering::Relaxed) {
            body.push_str(&format!("\n\n{}:\n{details}", t("error.details")));
        }
        let title = format!("fastdash - {}", t("error.title"));
        platform::message_box(&title, &body);
    }));
}

fn details(info: &PanicHookInfo<'_>) -> String {
    let message = info
        .payload()
        .downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| info.payload().downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown panic".into());
    let location = info
        .location()
        .map(|l| format!("{}:{}", l.file(), l.line()))
        .unwrap_or_default();
    format!(
        "fastdash {} panicked at {location}: {message}\n\n{}",
        env!("CARGO_PKG_VERSION"),
        Backtrace::force_capture()
    )
}

fn write_report(details: &str) {
    let Some(dirs) = ProjectDirs::from("co", "zro", "fastdash") else {
        return;
    };
    let path = dirs.config_dir().join("crash.log");
    let _ = std::fs::create_dir_all(dirs.config_dir());
    if let Ok(mut file) = std::fs::File::create(path) {
        let _ = writeln!(file, "{}\n\n{details}", chrono::Utc::now().to_rfc3339());
    }
}
