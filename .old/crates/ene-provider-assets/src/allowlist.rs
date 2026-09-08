use url::Url;

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
    let Ok(parsed) = Url::parse(url) else {
        return false;
    };
    parsed.scheme() == "https"
        && parsed
            .host_str()
            .is_some_and(|host| ALLOWED_HOSTS.contains(&host))
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
        assert!(is_allowed_url(
            "https://huggingface.co/ggml-org/models/resolve/main/model.gguf"
        ));
        assert!(is_allowed_url(
            "https://cdn-lfs.huggingface.co/repos/aa/bb/cc"
        ));
    }

    #[test]
    fn allows_https_on_allowed_host_with_default_port() {
        assert!(is_allowed_url("https://github.com:443/x"));
    }

    #[test]
    fn rejects_other_hosts() {
        assert!(!is_allowed_url("https://evil.invalid/x"));
        assert!(!is_allowed_url("http://github.com/x"));
        assert!(!is_allowed_url("https://github.com.evil.invalid/x"));
        assert!(!is_allowed_url("https://notgithub.com/x"));
    }

    #[test]
    fn rejects_userinfo_host_confusion() {
        assert!(!is_allowed_url("https://github.com@evil.invalid/x"));
        assert!(!is_allowed_url(
            "https://github.com%40evil.invalid/releases/download/x"
        ));
    }

    #[test]
    fn host_is_taken_from_parsed_hostname_not_userinfo() {
        assert!(is_allowed_url(
            "https://token:x@github.com/org/repo/releases/download/x"
        ));
        assert!(!is_allowed_url("https://github.com:x@evil.invalid/x"));
    }

    #[test]
    fn rejects_malformed_and_non_https() {
        assert!(!is_allowed_url(""));
        assert!(!is_allowed_url("github.com/x"));
        assert!(!is_allowed_url("ftp://github.com/x"));
        assert!(!is_allowed_url("https://"));
        assert!(!is_allowed_url("https:///no-host"));
        assert!(!is_allowed_url("https://127.0.0.1/x"));
    }
}
