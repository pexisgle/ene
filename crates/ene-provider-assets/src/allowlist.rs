const ALLOWED_HOSTS: &[&str] = &[
    "github.com",
    "release-assets.githubusercontent.com",
    "objects.githubusercontent.com",
    "huggingface.co",
    "cdn-lfs.huggingface.co",
];

/// Returns true when `url` is an HTTPS asset URL from an allowed GitHub host.
#[must_use]
pub fn is_allowed_url(url: &str) -> bool {
    if !url.starts_with("https://") {
        return false;
    }
    let rest = url.strip_prefix("https://").unwrap_or(url);
    let host = rest.split('/').next().unwrap_or("");
    let host = host.split(':').next().unwrap_or(host);
    ALLOWED_HOSTS.contains(&host)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allows_github_release_urls() {
        assert!(is_allowed_url(
            "https://github.com/ggml-org/llama.cpp/releases/download/b4282/llama-b4282-bin-win-avx2-x64.zip"
        ));
        assert!(is_allowed_url(
            "https://release-assets.githubusercontent.com/github-production-release-asset/612354784/foo.zip"
        ));
    }

    #[test]
    fn rejects_other_hosts() {
        assert!(!is_allowed_url("https://evil.invalid/x"));
        assert!(!is_allowed_url("http://github.com/x"));
    }
}
