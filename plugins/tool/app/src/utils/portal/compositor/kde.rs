use std::process::Command;

use ene_plugin_proto::ToolError;

use super::{WlCompositor, gvariant_string, js_string_literal, parse_gdbus_tuple_string};

// ==================== Version detection ====================

fn is_kde6() -> bool {
    std::env::var("KDE_SESSION_VERSION").is_ok_and(|v| v == "6")
}

// ==================== D-Bus transport abstraction ====================

/// D-Bus CLI transport used to talk to `KWin`'s scripting interface.
///
/// `qdbus` is the Qt front-end (preferred on Plasma); `gdbus` is the `GLib`
/// front-end used as a fallback. Both expose the same methods but differ in
/// invocation syntax and output formatting, which this enum encapsulates.
#[derive(Clone, Copy)]
enum DbusTransport {
    Qdbus,
    Gdbus,
}

impl DbusTransport {
    fn label(self) -> &'static str {
        match self {
            Self::Qdbus => "qdbus",
            Self::Gdbus => "gdbus",
        }
    }

    /// Builds a `Command` that invokes `method` on `object_path` with the given
    /// string `args`, using the correct syntax for this transport.
    fn call(self, object_path: &str, method: &str, args: &[&str]) -> Command {
        match self {
            Self::Qdbus => {
                let mut cmd = Command::new("qdbus");
                cmd.arg("org.kde.KWin").arg(object_path).arg(method);
                cmd.args(args);
                cmd
            }
            Self::Gdbus => {
                let mut cmd = Command::new("gdbus");
                cmd.arg("call")
                    .arg("--session")
                    .arg("--dest")
                    .arg("org.kde.KWin")
                    .arg("--object-path")
                    .arg(object_path)
                    .arg("--method")
                    .arg(method);
                for arg in args {
                    cmd.arg(gvariant_string(arg));
                }
                cmd
            }
        }
    }

    /// Extracts the integer script ID from `loadScript` stdout.
    ///
    /// `qdbus` prints the bare integer; `gdbus` wraps it in a `GVariant` tuple
    /// like `(42,)`.
    fn parse_script_id(self, stdout: &str) -> Option<i32> {
        match self {
            Self::Qdbus => stdout.trim().parse().ok(),
            Self::Gdbus => stdout
                .trim()
                .trim_start_matches('(')
                .trim_end_matches(')')
                .split(',')
                .next()?
                .trim()
                .parse()
                .ok(),
        }
    }
}

// ==================== Plasma 6: loadScript + print/journal ====================

const KWIN_MARKER: &str = "ENE_KWIN_OUT:";

fn capture_kwin_print_output(since_epoch_secs: u64) -> Result<String, ToolError> {
    let output = std::process::Command::new("journalctl")
        .args([
            "--no-pager",
            "-o",
            "cat",
            &format!("--since=@{since_epoch_secs}"),
        ])
        .output()
        .map_err(|e| ToolError::execution_failed(format!("Failed to run journalctl: {e}")))?;

    if !output.status.success() {
        return Err(ToolError::execution_failed(format!(
            "journalctl failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let lines: Vec<&str> = stdout
        .lines()
        .filter_map(|line| line.strip_prefix(KWIN_MARKER))
        .collect();

    Ok(lines.join("\n"))
}

fn kwin_load_and_run(transport: DbusTransport, js: &str) -> Result<String, ToolError> {
    let label = transport.label();
    let script_name = format!("ene-{}", std::process::id());
    let tmp_dir = std::env::temp_dir();
    let script_path = tmp_dir.join(format!("{script_name}.js"));
    std::fs::write(&script_path, js)
        .map_err(|e| ToolError::execution_failed(format!("Failed to write KWin script: {e}")))?;

    let path_str = script_path.to_string_lossy().to_string();
    let since_ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let output = transport
        .call(
            "/Scripting",
            "org.kde.kwin.Scripting.loadScript",
            &[&path_str, &script_name],
        )
        .output()
        .map_err(|e| {
            ToolError::execution_failed(format!("Failed to run {label} loadScript: {e}"))
        })?;

    if !output.status.success() {
        let _ = std::fs::remove_file(&script_path);
        return Err(ToolError::execution_failed(format!(
            "{label} loadScript failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let script_id = transport.parse_script_id(&stdout).ok_or_else(|| {
        ToolError::execution_failed(format!("Failed to parse script ID from: {stdout}"))
    })?;

    if script_id < 0 {
        let _ = std::fs::remove_file(&script_path);
        return Err(ToolError::execution_failed(
            "KWin loadScript returned negative ID".to_string(),
        ));
    }

    let script_obj = format!("/Scripting/Script{script_id}");
    let run_output = transport
        .call(&script_obj, "org.kde.kwin.Script.run", &[])
        .output()
        .map_err(|e| ToolError::execution_failed(format!("Failed to run {label} run: {e}")))?;

    std::thread::sleep(std::time::Duration::from_millis(300));

    if let Err(e) = transport
        .call(&script_obj, "org.kde.kwin.Script.stop", &[])
        .output()
    {
        tracing::warn!("Failed to stop KWin script ({label}): {e}");
    }

    if let Err(e) = transport
        .call(
            "/Scripting",
            "org.kde.kwin.Scripting.unloadScript",
            &[&script_name],
        )
        .output()
    {
        tracing::warn!("Failed to unload KWin script ({label}): {e}");
    }

    let _ = std::fs::remove_file(&script_path);

    if !run_output.status.success() {
        return Err(ToolError::execution_failed(format!(
            "{label} run failed: {}",
            String::from_utf8_lossy(&run_output.stderr)
        )));
    }

    capture_kwin_print_output(since_ts)
}

// ==================== Plasma 5: evaluateScript (legacy) ====================

fn kwin_eval(transport: DbusTransport, js: &str) -> Result<String, ToolError> {
    let label = transport.label();
    let output = transport
        .call("/Scripting", "org.kde.KWin.Scripting.evaluateScript", &[js])
        .output()
        .map_err(|e| ToolError::execution_failed(format!("Failed to run {label}: {e}")))?;

    if !output.status.success() {
        return Err(ToolError::execution_failed(format!(
            "{label} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    match transport {
        DbusTransport::Qdbus => Ok(stdout.trim().to_string()),
        DbusTransport::Gdbus => parse_gdbus_tuple_string(&stdout)
            .ok_or_else(|| ToolError::execution_failed("Failed to parse gdbus output".to_string())),
    }
}

// ==================== Transport fallback helpers ====================

/// Runs `f` with `qdbus`, falling back to `gdbus` when qdbus fails or yields
/// an empty result. The qdbus error is logged so the fallback is observable.
fn with_dbus_fallback<F>(mut f: F) -> Result<String, ToolError>
where
    F: FnMut(DbusTransport) -> Result<String, ToolError>,
{
    match f(DbusTransport::Qdbus) {
        Ok(result) if !result.is_empty() => return Ok(result),
        Ok(_) => {}
        Err(e) => {
            tracing::debug!("kwin qdbus failed, falling back to gdbus: {e}");
        }
    }
    f(DbusTransport::Gdbus)
}

// ==================== WlCompositor impl ====================

pub(super) struct Kde;

impl WlCompositor for Kde {
    fn list_windows(&self) -> Result<String, ToolError> {
        if is_kde6() {
            let js = "workspace.windowList().forEach(function(w){if(w.caption)print('ENE_KWIN_OUT:' + w.caption + ' | ' + (w.resourceClass || ''))})";
            return with_dbus_fallback(|t| kwin_load_and_run(t, js));
        }

        let js = "workspace.clientList().map(function(c){return c.caption + ' | ' + (c.resourceClass || '')}).join('\\n')";
        with_dbus_fallback(|t| kwin_eval(t, js))
    }

    fn focus_window(&self, title: &str) -> Result<String, ToolError> {
        let title_literal = js_string_literal(title);

        if is_kde6() {
            let js = format!(
                "var ws=workspace.windowList();for(var i=0;i<ws.length;i++){{if(ws[i].caption.indexOf({title_literal})!=-1||(ws[i].resourceClass||'').indexOf({title_literal})!=-1){{workspace.activeWindow=ws[i];break}}}}"
            );
            with_dbus_fallback(|t| kwin_load_and_run(t, &js))?;
            return Ok(format!("Focused window matching: {title}"));
        }

        let js = format!(
            "var cs=workspace.clientList();for(var i=0;i<cs.length;i++){{if(cs[i].caption.indexOf({title_literal})!=-1||(cs[i].resourceClass||'').indexOf({title_literal})!=-1){{workspace.activeWindow=cs[i];break}}}}"
        );
        with_dbus_fallback(|t| kwin_eval(t, &js))?;
        Ok(format!("Focused window matching: {title}"))
    }
}
