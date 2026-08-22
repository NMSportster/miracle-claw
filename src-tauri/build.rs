use std::time::{SystemTime, UNIX_EPOCH};

fn main() {
    tauri_build::build();

    // Lesson 528: emit a BUILD_TIMESTAMP so the EXE identifies itself in the
    // side-car log on startup. Format: "YYYY-MM-DD-HHMM" UTC — sortable, compact,
    // and tells us whether we're running the build we think we're running.
    // Used by lib.rs (miracle-claw.exe) and launcher.rs (miracle-claw-launcher.exe).
    //
    // No chrono dep — pure std to avoid bloating the build chain.
    let build_ts = unix_timestamp_to_compact(SystemTime::now());
    println!("cargo:rustc-env=BUILD_TIMESTAMP={}", build_ts);

    // Rebuild if build.rs itself changes.
    println!("cargo:rerun-if-changed=build.rs");
}

/// Format a SystemTime as "YYYY-MM-DD-HHMM" UTC. Compact enough for a log line.
fn unix_timestamp_to_compact(t: SystemTime) -> String {
    let secs = t
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    // Civil-from-days algorithm (Howard Hinnant's date.h) — pure integer math.
    let z = secs / 86400 + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    let h = (secs % 86400) / 3600;
    let mi = (secs % 3600) / 60;
    format!(
        "{:04}-{:02}-{:02}-{:02}{:02}",
        y,
        m,
        d,
        h,
        mi
    )
}