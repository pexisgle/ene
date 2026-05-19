use std::path::PathBuf;

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
        for dir in path_env.split(if cfg!(windows) { ';' } else { ':' }) {
            for candidate in candidates {
                let path = std::path::Path::new(dir).join(candidate);
                if path.is_file() {
                    return Some(path);
                }
            }
        }
    }

    let common_paths = &[
        "/usr/bin/google-chrome",
        "/usr/bin/google-chrome-stable",
        "/usr/bin/chromium",
        "/usr/bin/chromium-browser",
        "/usr/local/bin/google-chrome",
        "/usr/local/bin/chromium",
        "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
        "/Applications/Chromium.app/Contents/MacOS/Chromium",
    ];
    for path in common_paths {
        let p = PathBuf::from(path);
        if p.is_file() {
            return Some(p);
        }
    }

    None
}
