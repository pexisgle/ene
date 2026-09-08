//! CJK fallback fonts for Slint chrome and overlay UI.

use std::path::{Path, PathBuf};

/// Paths searched for a Japanese-capable UI font.
///
/// Optional `assets/fonts/NotoSansJP-Regular.ttf` next to the binary is preferred
/// when present; otherwise the OS CJK fonts (Yu Gothic / Meiryo on Windows, Noto / Droid on
/// Linux) are used as glyph fallbacks so labels are not tofu boxes.
#[must_use]
pub fn cjk_font_candidates() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    let bundled = [
        Path::new("assets/fonts/NotoSansJP-Regular.ttf"),
        Path::new("assets/fonts/NotoSansJP-Regular.otf"),
        Path::new("assets/fonts/NotoSansCJKjp-Regular.otf"),
    ];
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf));
    for rel in bundled {
        paths.push(rel.to_path_buf());
        if let Some(dir) = &exe_dir {
            paths.push(dir.join(rel));
            if let Some(parent) = dir.parent() {
                paths.push(parent.join(rel));
            }
        }
    }
    paths.extend(os_cjk_font_paths());
    paths
}

fn os_cjk_font_paths() -> Vec<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        let windir =
            std::env::var_os("WINDIR").map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from);
        let fonts = windir.join("Fonts");
        [
            "YuGothR.ttc",
            "YuGothM.ttc",
            "YuGothB.ttc",
            "yugothic.ttf",
            "meiryo.ttc",
            "msgothic.ttc",
            "msmincho.ttc",
            "malgun.ttf",
            "msyh.ttc",
            "msyhbd.ttc",
        ]
        .into_iter()
        .map(|name| fonts.join(name))
        .collect()
    }
    #[cfg(not(target_os = "windows"))]
    {
        [
            "/usr/share/fonts/truetype/droid/DroidSansFallbackFull.ttf",
            "/usr/share/fonts/truetype/wqy/wqy-microhei.ttc",
            "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/opentype/noto/NotoSansCJKjp-Regular.otf",
            "/usr/share/fonts/truetype/noto-cjk/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/opentype/noto-cjk/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/truetype/fonts-japanese-gothic.ttf",
            "/usr/share/fonts/truetype/takao-gothic/TakaoPGothic.ttf",
            "/usr/share/fonts/truetype/noto/NotoSansJP-Regular.ttf",
            "/usr/share/fonts/opentype/ipafont-gothic/ipag.otf",
        ]
        .into_iter()
        .map(PathBuf::from)
        .collect()
    }
}

/// First existing CJK font path, if any. Slint/FemtoVG also resolve
/// `Noto Sans CJK JP` via the platform fontconfig / DirectWrite stack.
#[must_use]
pub fn first_available_cjk_font() -> Option<PathBuf> {
    cjk_font_candidates()
        .into_iter()
        .find(|path| path.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn candidates_include_bundled_noto_sans_jp() {
        let paths = cjk_font_candidates();
        assert!(
            paths
                .iter()
                .any(|path| path.ends_with("NotoSansJP-Regular.ttf")),
            "bundled NotoSansJP path missing from {paths:?}"
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_candidates_include_yu_gothic_and_meiryo() {
        let joined = cjk_font_candidates()
            .iter()
            .map(|path| {
                path.to_string_lossy()
                    .replace('\\', "/")
                    .to_ascii_lowercase()
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(joined.contains("yugothr.ttc") || joined.contains("yugothic.ttf"));
        assert!(joined.contains("meiryo.ttc"));
        assert!(joined.contains("msgothic.ttc"));
    }
}
