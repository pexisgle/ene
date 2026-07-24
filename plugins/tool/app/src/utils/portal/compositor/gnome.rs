use ene_plugin_proto::ToolError;

use super::{WlCompositor, gvariant_string, js_string_literal, parse_gdbus_tuple_string};

pub(super) struct Gnome;

impl WlCompositor for Gnome {
    fn list_windows(&self) -> Result<String, ToolError> {
        let js = "global.get_window_actors().map(function(a){var w=a.meta_window;return w.get_title()+' | '+w.get_wm_class()}).join('\\n')";

        let output = std::process::Command::new("gdbus")
            .args([
                "call",
                "--session",
                "--dest",
                "org.gnome.Shell",
                "--object-path",
                "/org/gnome/Shell",
                "--method",
                "org.gnome.Shell.Eval",
                &gvariant_string(js),
            ])
            .output()
            .map_err(|e| ToolError::execution_failed(format!("Failed to run gdbus: {e}")))?;

        if !output.status.success() {
            return Err(ToolError::execution_failed(format!(
                "gdbus failed: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let result = parse_gdbus_tuple_string(&stdout).ok_or_else(|| {
            ToolError::execution_failed("Failed to parse gdbus output".to_string())
        })?;

        let mut windows = Vec::new();
        for line in result.lines() {
            let line = line.trim();
            if !line.is_empty() {
                windows.push(line.to_string());
            }
        }
        Ok(windows.join("\n"))
    }

    fn focus_window(&self, title: &str) -> Result<String, ToolError> {
        let title_literal = js_string_literal(title);
        let js = format!(
            "global.get_window_actors().forEach(function(a){{var w=a.meta_window;if(w.get_title().indexOf({title_literal})!=-1||w.get_wm_class().indexOf({title_literal})!=-1){{w.activate(global.get_current_time());w.get_workspace().activate(global.get_current_time())}}}})"
        );

        let output = std::process::Command::new("gdbus")
            .args([
                "call",
                "--session",
                "--dest",
                "org.gnome.Shell",
                "--object-path",
                "/org/gnome/Shell",
                "--method",
                "org.gnome.Shell.Eval",
                &gvariant_string(&js),
            ])
            .output()
            .map_err(|e| ToolError::execution_failed(format!("Failed to run gdbus: {e}")))?;

        if !output.status.success() {
            return Err(ToolError::execution_failed(format!(
                "gdbus focus failed: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let result = parse_gdbus_tuple_string(&stdout).ok_or_else(|| {
            ToolError::execution_failed("Failed to parse gdbus output".to_string())
        })?;

        if result == "not found" || result.is_empty() {
            return Err(ToolError::execution_failed(format!(
                "Window not found: {title}"
            )));
        }

        Ok(format!("Focused window matching: {title}"))
    }
}
