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

#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    reason = "test fixtures use unwrap for temp file setup"
)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// Sets an env var. Test-only; callers must hold [`ENV_LOCK`].
    fn set_env(key: &'static str, val: &str) {
        // SAFETY: serialized by ENV_LOCK; no concurrent readers of these vars.
        unsafe { std::env::set_var(key, val) }
    }

    /// Removes an env var. Test-only; callers must hold [`ENV_LOCK`].
    fn remove_env(key: &'static str) {
        // SAFETY: serialized by ENV_LOCK; no concurrent readers of these vars.
        unsafe { std::env::remove_var(key) }
    }

    #[test]
    fn finds_chrome_via_env_var() {
        let _guard = ENV_LOCK.lock().unwrap();
        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        tmp.write_all(b"#!/bin/sh\n").unwrap();
        let path = tmp.path().to_path_buf();

        set_env("CHROME_EXECUTABLE", path.to_str().unwrap());
        let result = find_chrome_executable();
        remove_env("CHROME_EXECUTABLE");

        assert_eq!(result, Some(path));
    }

    #[test]
    fn skips_nonexistent_env_var_path() {
        let _guard = ENV_LOCK.lock().unwrap();
        let fake = PathBuf::from("/nonexistent/chrome-binary-xyz");

        set_env("CHROME_EXECUTABLE", fake.to_str().unwrap());
        set_env("PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH", "");
        set_env("CHROMIUM_EXECUTABLE", "");
        let result = find_chrome_executable();
        remove_env("CHROME_EXECUTABLE");
        remove_env("PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH");
        remove_env("CHROMIUM_EXECUTABLE");

        // Must not return the nonexistent path (may return a system chrome or None).
        assert_ne!(result, Some(fake));
    }

    #[test]
    fn prefers_first_env_var_in_priority_order() {
        let _guard = ENV_LOCK.lock().unwrap();
        let mut tmp1 = tempfile::NamedTempFile::new().unwrap();
        tmp1.write_all(b"#!/bin/sh\n").unwrap();
        let mut tmp2 = tempfile::NamedTempFile::new().unwrap();
        tmp2.write_all(b"#!/bin/sh\n").unwrap();
        let path1 = tmp1.path().to_path_buf();
        let path2 = tmp2.path().to_path_buf();

        set_env(
            "PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH",
            path1.to_str().unwrap(),
        );
        set_env("CHROME_EXECUTABLE", path2.to_str().unwrap());
        let result = find_chrome_executable();
        remove_env("PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH");
        remove_env("CHROME_EXECUTABLE");

        assert_eq!(result, Some(path1));
    }
}
