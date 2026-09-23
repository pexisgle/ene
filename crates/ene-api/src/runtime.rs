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
    use super::{HOST_RUNTIME_FILE_NAME, HostRuntimeInfo};

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
        assert_eq!(info("wss://localhost:43121").local_port(), None);
        assert_eq!(info("wss://192.168.1.9:43121").local_port(), None);
        assert_eq!(info("http://127.0.0.1:43121").local_port(), None);
        assert_eq!(info("wss://127.0.0.1:0").local_port(), None);
        assert_eq!(info("wss://127.0.0.1:not-a-port").local_port(), None);
    }

    #[test]
    fn debug_redacts_the_local_token() {
        let rendered = format!("{:?}", info("wss://127.0.0.1:43121"));
        assert!(!rendered.contains("token-marker-9921"), "{rendered}");
        assert!(rendered.contains("[redacted]"), "{rendered}");
        assert!(rendered.contains("wss://127.0.0.1:43121"), "{rendered}");
    }

    #[test]
    fn the_runtime_file_roundtrips() {
        let original = info("wss://127.0.0.1:43121");
        let json = serde_json::to_string(&original).expect("runtime must serialize");
        let back: HostRuntimeInfo = serde_json::from_str(&json).expect("runtime must parse");
        assert_eq!(back, original, "runtime facts must survive the file");
        assert_eq!(HOST_RUNTIME_FILE_NAME, "host-runtime.json");
    }
}
