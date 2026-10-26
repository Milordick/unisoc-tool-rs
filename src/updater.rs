//! In-app self-updater for unisoc-tool.
//!
//! Checks GitHub Releases for new versions and downloads / applies updates.

use anyhow::{anyhow, Result};
use std::process::Command;

pub const REPO: &str = "Milordick/unisoc-tool-rs";
pub const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Information about a GitHub release
#[allow(dead_code)]
pub struct ReleaseInfo {
    pub tag: String,
    pub version: String,
    pub body: String,
    pub html_url: String,
}

/// Fetch latest release metadata from GitHub Releases API
pub fn fetch_latest_release() -> Result<ReleaseInfo> {
    let api_url = format!("https://api.github.com/repos/{}/releases/latest", REPO);

    // Try fetching via curl
    let output = Command::new("curl")
        .args([
            "-sSL",
            "-H",
            "User-Agent: unisoc-tool-updater",
            "-H",
            "Accept: application/vnd.github.v3+json",
            &api_url,
        ])
        .output();

    let json_text = match output {
        Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout).to_string(),
        _ => {
            // Windows PowerShell fallback
            if cfg!(windows) {
                let ps_cmd = format!(
                    "(Invoke-RestMethod -Uri '{}' -Headers @{{ 'User-Agent' = 'unisoc-tool-updater' }}) | ConvertTo-Json -Depth 2",
                    api_url
                );
                let ps_out = Command::new("powershell")
                    .args(["-NoProfile", "-Command", &ps_cmd])
                    .output()?;
                String::from_utf8_lossy(&ps_out.stdout).to_string()
            } else {
                return Err(anyhow!("Failed to execute curl to fetch update metadata"));
            }
        }
    };

    if json_text.contains("\"message\": \"Not Found\"") {
        return Err(anyhow!("No published releases found yet on repository {}", REPO));
    }

    let tag = extract_json_str(&json_text, "tag_name")
        .ok_or_else(|| anyhow!("Could not parse 'tag_name' from release response"))?;
    let version = tag.trim_start_matches('v').to_string();
    let body = extract_json_str(&json_text, "body").unwrap_or_default();
    let html_url = extract_json_str(&json_text, "html_url").unwrap_or_else(|| format!("https://github.com/{}", REPO));

    Ok(ReleaseInfo {
        tag,
        version,
        body,
        html_url,
    })
}

/// Simple JSON string extractor without pulling heavy serde_json
fn extract_json_str(json: &str, field: &str) -> Option<String> {
    let pattern = format!("\"{}\":", field);
    let start_pos = json.find(&pattern)? + pattern.len();
    let rest = json[start_pos..].trim_start();
    if !rest.starts_with('"') {
        return None;
    }
    let inner = &rest[1..];
    let mut end = 0;
    let mut escaped = false;
    for (i, c) in inner.char_indices() {
        if escaped {
            escaped = false;
        } else if c == '\\' {
            escaped = true;
        } else if c == '"' {
            end = i;
            break;
        }
    }
    if end > 0 {
        Some(inner[..end].replace("\\n", "\n").replace("\\\"", "\""))
    } else {
        None
    }
}

/// Compares semver versions "0.1.0" vs "0.1.1".
/// Returns true if `remote` is newer than `local`.
pub fn is_newer(remote: &str, local: &str) -> bool {
    let parse_nums = |s: &str| -> Vec<u64> {
        s.split('.')
            .filter_map(|part| part.trim().parse::<u64>().ok())
            .collect()
    };
    let r_nums = parse_nums(remote);
    let l_nums = parse_nums(local);

    for (r, l) in r_nums.iter().zip(l_nums.iter()) {
        if r > l {
            return true;
        }
        if r < l {
            return false;
        }
    }
    r_nums.len() > l_nums.len()
}

/// Checks for and executes an in-place self-update
pub fn run_self_update(force: bool) -> Result<()> {
    println!("🔍 Проверка обновлений на GitHub ({}) ...", REPO);
    println!("Текущая версия программы: v{}", CURRENT_VERSION);

    let release = match fetch_latest_release() {
        Ok(r) => r,
        Err(e) => {
            println!("ℹ️  Не удалось проверить обновления: {}", e);
            println!("Вы можете скачать последнюю версию вручную: https://github.com/{}", REPO);
            return Ok(());
        }
    };

    let needs_update = force || is_newer(&release.version, CURRENT_VERSION);

    if !needs_update {
        println!("✅ У вас установлена актуальная версия (v{}). Обновление не требуется.", CURRENT_VERSION);
        return Ok(());
    }

    println!("\n🚀 Найдена новая версия: {} (текущая: v{})", release.tag, CURRENT_VERSION);
    if !release.body.trim().is_empty() {
        println!("--------------------------------------------------");
        println!("Список изменений (Release Notes):\n{}", release.body.trim());
        println!("--------------------------------------------------");
    }

    println!("\n⏳ Запуск процесса автоматического обновления...");

    if cfg!(windows) {
        let ps_cmd = "irm https://raw.githubusercontent.com/Milordick/unisoc-tool-rs/main/install.ps1 | iex";
        let status = Command::new("powershell")
            .args(["-NoProfile", "-Command", ps_cmd])
            .status()?;
        if status.success() {
            println!("\n✅ Обновление успешно установлено! Перезапустите программу.");
        } else {
            println!("\n❌ Ошибка при выполнении скрипта обновления.");
        }
    } else {
        let shell_cmd = "curl -fsSL https://raw.githubusercontent.com/Milordick/unisoc-tool-rs/main/install.sh | bash";
        let status = Command::new("sh").arg("-c").arg(shell_cmd).status()?;
        if status.success() {
            println!("\n✅ Обновление успешно установлено! Перезапустите программу.");
        } else {
            println!("\n❌ Ошибка при выполнении скрипта обновления.");
        }
    }

    Ok(())
}
