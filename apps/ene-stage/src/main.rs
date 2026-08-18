#![expect(
    clippy::print_stderr,
    reason = "stage reports connection failures on stderr before the window opens"
)]
#![deny(unsafe_code)]

mod core_spawn;
mod filter;
mod stage_app;
mod vrm;

use std::path::PathBuf;
use std::sync::Arc;

use eframe::egui::ViewportBuilder;
use stage_app::StageApp;

fn default_minimal_vrm() -> Option<PathBuf> {
    let dir = std::env::temp_dir().join("ene-stage-vrm");
    std::fs::create_dir_all(&dir).ok()?;
    let path = dir.join("minimal.vrm");
    if !path.is_file() {
        ene_vrm::minimal::write_glb(&path).ok()?;
    }
    Some(path)
}

fn wgpu_options_for_vrm() -> egui_wgpu::WgpuConfiguration {
    let mut options = egui_wgpu::WgpuConfiguration::default();
    if let egui_wgpu::WgpuSetup::CreateNew(create) = &mut options.wgpu_setup {
        create.device_descriptor = Arc::new(|adapter| {
            let adapter_limits = adapter.limits();
            let mut limits = if adapter.get_info().backend == wgpu::Backend::Gl {
                wgpu::Limits::downlevel_webgl2_defaults()
            } else {
                wgpu::Limits::default()
            };
            limits.max_texture_dimension_2d = 8192;
            // Unlit VRM uses 5 bind groups and MToon uses 7; eframe's WebGPU
            // default device keeps max_bind_groups at 4 and panics on create.
            limits.max_bind_groups = adapter_limits.max_bind_groups;
            wgpu::DeviceDescriptor {
                label: Some("ene-stage wgpu device"),
                required_limits: limits,
                ..wgpu::DeviceDescriptor::default()
            }
        });
    }
    options
}

fn main() {
    let text_only = std::env::var("ENE_STAGE_TEXT_ONLY")
        .is_ok_and(|v| v == "1" || v.eq_ignore_ascii_case("true"));
    let vrm_path = std::env::var("ENE_VRM_PATH")
        .ok()
        .filter(|p| !p.is_empty())
        .map(PathBuf::from)
        .or_else(default_minimal_vrm);
    let native = eframe::NativeOptions {
        viewport: ViewportBuilder::default()
            .with_title("ene stage")
            .with_inner_size([1280.0, 720.0]),
        wgpu_options: wgpu_options_for_vrm(),
        ..Default::default()
    };
    if let Err(err) = eframe::run_native(
        "ene stage",
        native,
        Box::new(move |cc| Ok(Box::new(StageApp::new(cc, text_only, vrm_path)))),
    ) {
        eprintln!("ene-stage: {err}");
        std::process::exit(1);
    }
}

#[cfg(test)]
#[expect(
    clippy::expect_used,
    unsafe_code,
    reason = "unit tests assert concrete values and restore process environment"
)]
mod tests {
    use super::core_spawn::{
        binary_in_dir, connection_from_ready, env_api_config, health_reachable, spawn_core,
        wait_for_api_json,
    };
    use super::filter::{
        job_report_matches_soul, live_surface_line, merge_soul_ids, surface_event_allowed,
        surface_history_line,
    };
    use serde_json::json;
    use tempfile::TempDir;

    #[test]
    fn surface_blocks_inner_and_thinking() {
        assert!(!surface_event_allowed(
            &json!({"type": "inner.message", "text": "x"})
        ));
        assert!(!surface_event_allowed(
            &json!({"type": "thinking.delta", "text": "x"})
        ));
        assert!(surface_event_allowed(
            &json!({"type": "text.delta", "text": "hi"})
        ));
        assert!(!surface_event_allowed(
            &json!({"type": "session.event", "kind": "inner/message"})
        ));
        assert!(!surface_event_allowed(
            &json!({"type": "session.event", "kind": "assistant/thinking"})
        ));
        assert!(surface_event_allowed(
            &json!({"type": "session.event", "kind": "turn/end"})
        ));
        assert_eq!(
            live_surface_line(&json!({"type": "text.delta", "text": "hi"})).as_deref(),
            Some("assistant: hi")
        );
        assert_eq!(
            live_surface_line(&json!({"type": "job.report", "speech": "done — notes"})).as_deref(),
            Some("assistant: done — notes")
        );
        assert!(
            live_surface_line(&json!({"type": "session.event", "kind": "turn/end"})).is_none(),
            "session.event JSON must not land on the stage surface"
        );
        assert!(job_report_matches_soul(
            &json!({"type": "job.report", "soul_id": "a", "speech": "x"}),
            "a"
        ));
        assert!(!job_report_matches_soul(
            &json!({"type": "job.report", "soul_id": "a", "speech": "x"}),
            "b"
        ));
    }

    #[test]
    fn default_minimal_vrm_writes_parseable_glb() {
        let path = super::default_minimal_vrm().expect("fixture");
        let bytes = std::fs::read(&path).expect("read");
        assert!(bytes.starts_with(b"glTF"));
        assert!(bytes.len() > 12);
    }

    #[test]
    fn surface_history_skips_inner_role() {
        assert!(surface_history_line("inner", "secret").is_none());
        assert!(surface_history_line("system", "prompt").is_none());
        assert_eq!(
            surface_history_line("assistant", "hi").as_deref(),
            Some("assistant: hi")
        );
        assert_eq!(
            surface_history_line("user", "hello").as_deref(),
            Some("user: hello")
        );
    }

    #[test]
    fn merge_soul_ids_keeps_two_occupants_in_order() {
        let occupants = vec!["char.alpha@1".into(), "char.beta@1".into()];
        let extras = vec!["char.alpha@1".into(), "char.gamma@1".into()];
        let ids = merge_soul_ids(&occupants, &extras);
        assert_eq!(ids, vec!["char.alpha@1", "char.beta@1", "char.gamma@1"]);
        assert!(ids.len() >= 2);
    }

    #[test]
    fn stage_hosts_two_vrm_panes_and_text_only_flag() {
        let app = include_str!("stage_app.rs");
        assert!(app.contains("companions: [CompanionPane; 2]"));
        assert!(app.contains("vrm_left"));
        assert!(app.contains("vrm_right"));
        let main = include_str!("main.rs");
        assert!(main.contains("ENE_STAGE_TEXT_ONLY"));
        assert!(app.contains("text_only"));
    }

    #[test]
    fn wait_for_api_json_reads_url_and_token() {
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("api.json");
        std::fs::write(
            &path,
            r#"{"url":"http://127.0.0.1:9","token_file":"api.token","bind":"127.0.0.1:9"}"#,
        )
        .expect("write");
        std::fs::write(dir.path().join("api.token"), "abc").expect("token");
        let value = wait_for_api_json(&path).expect("ready");
        let (url, token) = connection_from_ready(&value).expect("parse");
        assert_eq!(url, "http://127.0.0.1:9");
        assert_eq!(token, "abc");
    }

    #[test]
    fn binary_in_dir_finds_sibling_named_ene_core() {
        let dir = TempDir::new().expect("tempdir");
        let bin = dir.path().join("ene-core");
        std::fs::write(&bin, b"#!/bin/sh\n").expect("write");
        assert_eq!(binary_in_dir(dir.path()), Some(bin));
    }

    #[test]
    fn env_api_config_rejects_empty_and_zero_port() {
        let saved = std::env::var("ENE_API_URL").ok();
        unsafe {
            std::env::set_var("ENE_API_URL", "");
        }
        assert!(env_api_config().is_none());
        unsafe {
            std::env::set_var("ENE_API_URL", "http://127.0.0.1:0");
        }
        assert!(env_api_config().is_none());
        unsafe {
            std::env::set_var("ENE_API_URL", "http://127.0.0.1:8080");
        }
        let (url, _) = env_api_config().expect("config");
        assert_eq!(url, "http://127.0.0.1:8080");
        match saved {
            Some(value) => unsafe {
                std::env::set_var("ENE_API_URL", value);
            },
            None => unsafe {
                std::env::remove_var("ENE_API_URL");
            },
        }
    }

    #[tokio::test]
    async fn spawn_core_writes_api_json_and_health_succeeds() {
        let Some(_) = super::core_spawn::ene_core_binary() else {
            return;
        };
        let dir = TempDir::new().expect("tempdir");
        let child = spawn_core(dir.path()).expect("spawn");
        let ready = wait_for_api_json(&dir.path().join("api.json")).expect("ready");
        let (url, token) = connection_from_ready(&ready).expect("connection");
        let client = ene_api::ApiClient::new(url, token, "stage");
        health_reachable(&client, std::time::Duration::from_secs(5))
            .await
            .expect("health");
        drop(child);
    }
}
