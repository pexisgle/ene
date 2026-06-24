use std::path::PathBuf;

const PATH_SEPARATOR: char = if cfg!(windows) { ';' } else { ':' };

pub fn find_chrome_executable() -> Option<PathBuf> {
    for env_var in &[
        "PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH",
        "CHROME_EXECUTABLE",
        "CHROMIUM_EXECUTABLE",
    ] {
        if let Ok(path) = std::env::var(env_var) {
            let p = PathBuf::from(path);
            if p.is_file() {
                return Some(p);
            }
        }
    }

    let candidates = &[
        "google-chrome",
        "google-chrome-stable",
        "chromium",
        "chromium-browser",
        "microsoft-edge",
        "brave",
    ];

    if let Ok(path_env) = std::env::var("PATH") {
        for dir in path_env.split(PATH_SEPARATOR) {
            for candidate in candidates {
                let path = std::path::Path::new(dir).join(candidate);
                if path.is_file() {
                    return Some(path);
                }
            }
        }
    }

    let common_paths: &[&str] = if cfg!(target_os = "macos") {
        // macOS is not a supported target (AGENTS.md §3).
        // The lookup falls through to the `PATH` scan
        // above, which already covers the standard
        // `/usr/bin/` and `/usr/local/bin/` installs.
        // Listed explicitly here would falsely advertise
        // support for the platform.
        &[]
    } else {
        &[
            "/usr/bin/google-chrome",
            "/usr/bin/google-chrome-stable",
            "/usr/bin/chromium",
            "/usr/bin/chromium-browser",
            "/usr/local/bin/google-chrome",
            "/usr/local/bin/chromium",
        ]
    };
    for path in common_paths {
        let p = PathBuf::from(path);
        if p.is_file() {
            return Some(p);
        }
    }

    None
}
