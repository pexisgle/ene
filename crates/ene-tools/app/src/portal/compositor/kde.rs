use ene_tool_proto::ToolError;

use super::{WlCompositor, gvariant_string, parse_gdbus_tuple_string};

// ==================== Version detection ====================

fn is_kde6() -> bool {
    std::env::var("KDE_SESSION_VERSION")
        .map(|v| v == "6")
        .unwrap_or(false)
}

// ==================== Plasma 6: loadScript + print/journal ====================

const KWIN_MARKER: &str = "ENE_KWIN_OUT:";

fn capture_kwin_print_output(since_epoch_secs: u64) -> Result<String, ToolError> {
    let output = std::process::Command::new("journalctl")
        .args([
            "--no-pager",
            "-o",
            "cat",
            &format!("--since=@{}", since_epoch_secs),
        ])
        .output()
        .map_err(|e| ToolError::ExecutionFailed {
            message: format!("Failed to run journalctl: {e}"),
        })?;

    if !output.status.success() {
        return Err(ToolError::ExecutionFailed {
            message: format!(
                "journalctl failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ),
        });
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let lines: Vec<&str> = stdout
        .lines()
        .filter_map(|line| line.strip_prefix(KWIN_MARKER))
        .collect();

    Ok(lines.join("\n"))
}

fn kwin_load_and_run_qdbus(js: &str) -> Result<String, ToolError> {
    let script_name = format!("ene-{}", std::process::id());
    let tmp_dir = std::env::temp_dir();
    let script_path = tmp_dir.join(format!("{}.js", script_name));
    std::fs::write(&script_path, js).map_err(|e| ToolError::ExecutionFailed {
        message: format!("Failed to write KWin script: {e}"),
    })?;

    let path_str = script_path.to_string_lossy().to_string();
    let since_ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let output = std::process::Command::new("qdbus")
        .args([
            "org.kde.KWin",
            "/Scripting",
            "org.kde.kwin.Scripting.loadScript",
            &path_str,
            &script_name,
        ])
        .output()
        .map_err(|e| ToolError::ExecutionFailed {
            message: format!("Failed to run qdbus loadScript: {e}"),
        })?;

    if !output.status.success() {
        let _ = std::fs::remove_file(&script_path);
        return Err(ToolError::ExecutionFailed {
            message: format!(
                "qdbus loadScript failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ),
        });
    }

    let script_id: i32 = String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse()
        .map_err(|_| ToolError::ExecutionFailed {
            message: format!(
                "Failed to parse script ID from: {}",
                String::from_utf8_lossy(&output.stdout)
            ),
        })?;

    if script_id < 0 {
        let _ = std::fs::remove_file(&script_path);
        return Err(ToolError::ExecutionFailed {
            message: "KWin loadScript returned negative ID".to_string(),
        });
    }

    let script_obj = format!("/Scripting/Script{}", script_id);
    let run_output = std::process::Command::new("qdbus")
        .args(["org.kde.KWin", &script_obj, "org.kde.kwin.Script.run"])
        .output()
        .map_err(|e| ToolError::ExecutionFailed {
            message: format!("Failed to run qdbus run: {e}"),
        })?;

    std::thread::sleep(std::time::Duration::from_millis(300));

    let _ = std::process::Command::new("qdbus")
        .args(["org.kde.KWin", &script_obj, "org.kde.kwin.Script.stop"])
        .output();

    let _ = std::process::Command::new("qdbus")
        .args([
            "org.kde.KWin",
            "/Scripting",
            "org.kde.kwin.Scripting.unloadScript",
            &script_name,
        ])
        .output();

    let _ = std::fs::remove_file(&script_path);

    if !run_output.status.success() {
        return Err(ToolError::ExecutionFailed {
            message: format!(
                "qdbus run failed: {}",
                String::from_utf8_lossy(&run_output.stderr)
            ),
        });
    }

    capture_kwin_print_output(since_ts)
}

fn kwin_load_and_run_gdbus(js: &str) -> Result<String, ToolError> {
    let script_name = format!("ene-{}", std::process::id());
    let tmp_dir = std::env::temp_dir();
    let script_path = tmp_dir.join(format!("{}.js", script_name));
    std::fs::write(&script_path, js).map_err(|e| ToolError::ExecutionFailed {
        message: format!("Failed to write KWin script: {e}"),
    })?;

    let path_str = script_path.to_string_lossy().to_string();
    let since_ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let load_output = std::process::Command::new("gdbus")
        .args([
            "call",
            "--session",
            "--dest",
            "org.kde.KWin",
            "--object-path",
            "/Scripting",
            "--method",
            "org.kde.kwin.Scripting.loadScript",
            &gvariant_string(&path_str),
            &gvariant_string(&script_name),
        ])
        .output()
        .map_err(|e| ToolError::ExecutionFailed {
            message: format!("Failed to run gdbus loadScript: {e}"),
        })?;

    if !load_output.status.success() {
        let _ = std::fs::remove_file(&script_path);
        return Err(ToolError::ExecutionFailed {
            message: format!(
                "gdbus loadScript failed: {}",
                String::from_utf8_lossy(&load_output.stderr)
            ),
        });
    }

    let stdout = String::from_utf8_lossy(&load_output.stdout).to_string();
    let script_id: i32 = stdout
        .trim()
        .trim_start_matches('(')
        .trim_end_matches(')')
        .split(',')
        .next()
        .unwrap_or("")
        .trim()
        .parse()
        .map_err(|_| ToolError::ExecutionFailed {
            message: format!("Failed to parse script ID from: {}", stdout),
        })?;

    if script_id < 0 {
        let _ = std::fs::remove_file(&script_path);
        return Err(ToolError::ExecutionFailed {
            message: "KWin loadScript returned negative ID".to_string(),
        });
    }

    let script_obj = format!("/Scripting/Script{}", script_id);
    let run_output = std::process::Command::new("gdbus")
        .args([
            "call",
            "--session",
            "--dest",
            "org.kde.KWin",
            "--object-path",
            &script_obj,
            "--method",
            "org.kde.kwin.Script.run",
        ])
        .output()
        .map_err(|e| ToolError::ExecutionFailed {
            message: format!("Failed to run gdbus run: {e}"),
        })?;

    std::thread::sleep(std::time::Duration::from_millis(300));

    let _ = std::process::Command::new("gdbus")
        .args([
            "call",
            "--session",
            "--dest",
            "org.kde.KWin",
            "--object-path",
            &script_obj,
            "--method",
            "org.kde.kwin.Script.stop",
        ])
        .output();

    let _ = std::process::Command::new("gdbus")
        .args([
            "call",
            "--session",
            "--dest",
            "org.kde.KWin",
            "--object-path",
            "/Scripting",
            "--method",
            "org.kde.kwin.Scripting.unloadScript",
            &gvariant_string(&script_name),
        ])
        .output();

    let _ = std::fs::remove_file(&script_path);

    if !run_output.status.success() {
        return Err(ToolError::ExecutionFailed {
            message: format!(
                "gdbus run failed: {}",
                String::from_utf8_lossy(&run_output.stderr)
            ),
        });
    }

    capture_kwin_print_output(since_ts)
}

// ==================== Plasma 5: evaluateScript (legacy) ====================

fn kwin_eval_qdbus(js: &str) -> Result<String, ToolError> {
    let output = std::process::Command::new("qdbus")
        .args(["org.kde.KWin", "/Scripting", "evaluateScript", js])
        .output()
        .map_err(|e| ToolError::ExecutionFailed {
            message: format!("Failed to run qdbus: {e}"),
        })?;

    if !output.status.success() {
        return Err(ToolError::ExecutionFailed {
            message: format!("qdbus failed: {}", String::from_utf8_lossy(&output.stderr)),
        });
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn kwin_eval_gdbus(js: &str) -> Result<String, ToolError> {
    let output = std::process::Command::new("gdbus")
        .args([
            "call",
            "--session",
            "--dest",
            "org.kde.KWin",
            "--object-path",
            "/Scripting",
            "--method",
            "org.kde.KWin.Scripting.evaluateScript",
            &gvariant_string(js),
        ])
        .output()
        .map_err(|e| ToolError::ExecutionFailed {
            message: format!("Failed to run gdbus: {e}"),
        })?;

    if !output.status.success() {
        return Err(ToolError::ExecutionFailed {
            message: format!("gdbus failed: {}", String::from_utf8_lossy(&output.stderr)),
        });
    }

    parse_gdbus_tuple_string(&String::from_utf8_lossy(&output.stdout)).ok_or_else(|| {
        ToolError::ExecutionFailed {
            message: "Failed to parse gdbus output".to_string(),
        }
    })
}

// ==================== WlCompositor impl ====================

pub(super) struct Kde;

impl WlCompositor for Kde {
    fn list_windows(&self) -> Result<String, ToolError> {
        if is_kde6() {
            let js = "workspace.windowList().forEach(function(w){if(w.caption)print('ENE_KWIN_OUT:' + w.caption + ' | ' + (w.resourceClass || ''))})";
            if let Ok(result) = kwin_load_and_run_qdbus(js)
                && !result.is_empty()
            {
                return Ok(result);
            }
            return kwin_load_and_run_gdbus(js);
        }

        let js = "workspace.clientList().map(function(c){return c.caption + ' | ' + (c.resourceClass || '')}).join('\\n')";
        if let Ok(result) = kwin_eval_qdbus(js)
            && !result.is_empty()
        {
            return Ok(result);
        }
        kwin_eval_gdbus(js)
    }

    fn focus_window(&self, title: &str) -> Result<String, ToolError> {
        let escaped = title.replace('\\', "\\\\").replace('\'', "\\'");

        if is_kde6() {
            let js = format!(
                "var ws=workspace.windowList();for(var i=0;i<ws.length;i++){{if(ws[i].caption.indexOf('{}')!=-1||(ws[i].resourceClass||'').indexOf('{}')!=-1){{workspace.activeWindow=ws[i];break}}}}",
                escaped, escaped
            );
            if let Ok(_result) = kwin_load_and_run_qdbus(&js) {
                return Ok(format!("Focused window matching: {}", title));
            }
            let _ = kwin_load_and_run_gdbus(&js)?;
            return Ok(format!("Focused window matching: {}", title));
        }

        let js = format!(
            "var cs=workspace.clientList();for(var i=0;i<cs.length;i++){{if(cs[i].caption.indexOf('{}')!=-1||(cs[i].resourceClass||'').indexOf('{}')!=-1){{workspace.activeWindow=cs[i];break}}}}",
            escaped, escaped
        );
        if let Ok(_result) = kwin_eval_qdbus(&js) {
            return Ok(format!("Focused window matching: {}", title));
        }
        let _ = kwin_eval_gdbus(&js)?;
        Ok(format!("Focused window matching: {}", title))
    }
}
