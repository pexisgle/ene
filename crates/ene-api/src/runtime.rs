use serde::{Deserialize, Serialize};

pub const HOST_RUNTIME_FILE_NAME: &str = "host-runtime.json";

/// Connection facts a serving Host publishes for local Clients.
///
/// The file is class T (secret-bearing, boot-limited): Owners publish and
/// remove it; Clients read it without repair.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostRuntimeInfo {
    pub url: String,
    pub host_pin: String,
    pub startup_generation: String,
    pub local_token: String,
}

impl core::fmt::Debug for HostRuntimeInfo {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("HostRuntimeInfo")
            .field("url", &self.url)
            .field("host_pin", &self.host_pin)
            .field("startup_generation", &self.startup_generation)
            .field("local_token", &"[redacted]")
            .finish()
    }
}

impl HostRuntimeInfo {
    #[must_use]
    pub fn local_port(&self) -> Option<u16> {
        let port = self.url.strip_prefix("wss://127.0.0.1:")?;
        let port = port.parse::<u16>().ok()?;
        (port != 0).then_some(port)
    }
}

#[cfg(test)]
mod tests {
    use super::HostRuntimeInfo;

    fn info(url: &str) -> HostRuntimeInfo {
        HostRuntimeInfo {
            url: url.to_owned(),
            host_pin: String::from("a1b2"),
            startup_generation: String::from("generation-1"),
            local_token: String::from("token-marker-9921"),
        }
    }

    #[test]
    fn only_a_local_wss_url_parses_as_a_port() {
        assert_eq!(info("wss://127.0.0.1:43121").local_port(), Some(43121));
        for url in [
            "wss://[::1]:43121",
            "wss://[::ffff:127.0.0.1]:43121",
            "wss://localhost:43121",
            "wss://127.0.0.2:43121",
            "wss://192.168.1.9:43121",
            "http://127.0.0.1:43121",
            "wss://127.0.0.1:0",
            "wss://127.0.0.1:65536",
            "wss://127.0.0.1:18446744073709551616",
            "wss://127.0.0.1:-1",
            "wss://127.0.0.1:not-a-port",
            "wss://127.0.0.1:43121x",
            "wss://127.0.0.1:43121/path",
            "wss://127.0.0.1:",
        ] {
            assert_eq!(info(url).local_port(), None, "{url}");
        }
    }
}
