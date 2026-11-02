//! yt-dlp, ffmpeg and Deno: install on first run, report versions, update.
//! Everything lives in the user's local app data folder, so no admin rights
//! are ever needed and yt-dlp can replace itself.

use crate::logic;
use futures_util::StreamExt;
use sha2::{Digest, Sha256};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, SystemTime};
use tauri::{AppHandle, Emitter, Manager};
use tokio::io::AsyncWriteExt;

const UA: &str = "Grabber (yt-dlp GUI)";
const YTDLP_STABLE: &str = "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp.exe";
const YTDLP_NIGHTLY: &str =
    "https://github.com/yt-dlp/yt-dlp-nightly-builds/releases/latest/download/yt-dlp.exe";
const FFMPEG_ZIP: &str =
    "https://github.com/yt-dlp/FFmpeg-Builds/releases/download/latest/ffmpeg-master-latest-win64-gpl.zip";
const DENO_ZIP: &str =
    "https://github.com/denoland/deno/releases/latest/download/deno-x86_64-pc-windows-msvc.zip";
// official checksum lists published next to each download
const YTDLP_STABLE_SUMS: &str = "https://github.com/yt-dlp/yt-dlp/releases/latest/download/SHA2-256SUMS";
const YTDLP_NIGHTLY_SUMS: &str =
    "https://github.com/yt-dlp/yt-dlp-nightly-builds/releases/latest/download/SHA2-256SUMS";
const FFMPEG_SUMS: &str = "https://github.com/yt-dlp/FFmpeg-Builds/releases/download/latest/checksums.sha256";
const DENO_SUMS: &str =
    "https://github.com/denoland/deno/releases/latest/download/deno-x86_64-pc-windows-msvc.zip.sha256sum";
const FFMPEG_MAX_AGE_DAYS: u64 = 60;

pub const TOOLS: [&str; 3] = ["yt-dlp", "ffmpeg", "deno"];

pub fn err<E: ToString>(e: E) -> String {
    e.to_string()
}

pub fn bin_dir(app: &AppHandle) -> PathBuf {
    app.path()
        .app_local_data_dir()
        .unwrap_or_else(|_| std::env::temp_dir().join("Grabber"))
        .join("bin")
}

pub fn tool_path(app: &AppHandle, name: &str) -> PathBuf {
    bin_dir(app).join(format!("{}{}", name, std::env::consts::EXE_SUFFIX))
}

/// A command that never opens a console window and finds our tools first.
pub fn command(app: &AppHandle, program: &Path) -> tokio::process::Command {
    let mut c = tokio::process::Command::new(program);
    #[cfg(windows)]
    {
        c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let mut paths = vec![bin_dir(app)];
    if let Some(p) = std::env::var_os("PATH") {
        paths.extend(std::env::split_paths(&p));
    }
    if let Ok(joined) = std::env::join_paths(paths) {
        c.env("PATH", joined);
    }
    c.env("PYTHONIOENCODING", "utf-8").env("PYTHONUTF8", "1");
    c.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    c
}

async fn run_capture(app: &AppHandle, program: &Path, args: &[&str]) -> Result<(bool, String, String), String> {
    let out = command(app, program).args(args).output().await.map_err(err)?;
    Ok((
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    ))
}

fn http() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent(UA)
        .connect_timeout(Duration::from_secs(20))
        .build()
        .expect("http client")
}

#[derive(Serialize, Clone)]
pub struct ToolInfo {
    pub name: String,
    pub installed: bool,
    pub version: Option<String>,
    pub latest: Option<String>,
    pub outdated: bool,
}

#[derive(Serialize, Clone)]
struct ToolProgress {
    name: String,
    stage: String,
    percent: f64,
}

fn emit(app: &AppHandle, name: &str, stage: &str, percent: f64) {
    let _ = app.emit(
        "tool-progress",
        ToolProgress { name: name.into(), stage: stage.into(), percent },
    );
}

async fn local_version(app: &AppHandle, name: &str) -> Option<String> {
    let p = tool_path(app, name);
    if !p.exists() {
        return None;
    }
    let arg = if name == "ffmpeg" { "-version" } else { "--version" };
    let (_, out, _) = run_capture(app, &p, &[arg]).await.ok()?;
    let first = out.lines().next()?.trim().to_string();
    match name {
        // "ffmpeg version N-12345-gabcdef-20261010 Copyright ..."
        "ffmpeg" => first.split_whitespace().nth(2).map(String::from),
        // "deno 2.5.4 (stable, release, x86_64-pc-windows-msvc)"
        "deno" => first.split_whitespace().nth(1).map(String::from),
        _ => Some(first),
    }
}

async fn github_latest(repo: &str) -> Option<String> {
    let v: serde_json::Value = http()
        .get(format!("https://api.github.com/repos/{}/releases/latest", repo))
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .ok()?
        .error_for_status()
        .ok()?
        .json()
        .await
        .ok()?;
    v["tag_name"].as_str().map(|t| t.trim_start_matches('v').to_string())
}

fn older_than(p: &Path, days: u64) -> bool {
    std::fs::metadata(p)
        .and_then(|m| m.modified())
        .map(|t| SystemTime::now().duration_since(t).unwrap_or_default() > Duration::from_secs(days * 86_400))
        .unwrap_or(false)
}

pub async fn status(app: &AppHandle, channel: &str, check_latest: bool) -> Vec<ToolInfo> {
    let mut v = Vec::new();
    for name in TOOLS {
        let path = tool_path(app, name);
        let installed = path.exists();
        let version = if installed { local_version(app, name).await } else { None };
        let latest = if check_latest && installed {
            match name {
                "yt-dlp" => {
                    github_latest(if channel == "nightly" {
                        "yt-dlp/yt-dlp-nightly-builds"
                    } else {
                        "yt-dlp/yt-dlp"
                    })
                    .await
                }
                "deno" => github_latest("denoland/deno").await,
                _ => None,
            }
        } else {
            None
        };
        let outdated = match name {
            "ffmpeg" => installed && older_than(&path, FFMPEG_MAX_AGE_DAYS),
            _ => matches!((&version, &latest), (Some(a), Some(b)) if a != b),
        };
        v.push(ToolInfo { name: name.into(), installed, version, latest, outdated });
    }
    v
}

async fn fetch_expected_hash(name: &str, sums_url: &str, file: &str) -> Result<String, String> {
    let text = http()
        .get(sums_url)
        .send()
        .await
        .and_then(|r| r.error_for_status())
        .map_err(|e| format!("{} 체크섬을 받지 못했어요: {}", name, e))?
        .text()
        .await
        .map_err(err)?;
    logic::expected_sha256(&text, file).ok_or_else(|| format!("{} 체크섬 목록에서 {}를 찾지 못했어요.", name, file))
}

/// Downloads `url` to `dest` and refuses to keep it unless its SHA-256
/// matches the official checksum list at `sums_url`.
async fn download_verified(app: &AppHandle, name: &str, url: &str, sums_url: &str, dest: &Path) -> Result<(), String> {
    let file = url.rsplit('/').next().unwrap_or(url);
    let expected = fetch_expected_hash(name, sums_url, file).await?;
    let resp = http()
        .get(url)
        .send()
        .await
        .map_err(|e| format!("{} 받기 실패: {}", name, e))?
        .error_for_status()
        .map_err(|e| format!("{} 받기 실패: {}", name, e))?;
    let total = resp.content_length().unwrap_or(0);
    let mut out = tokio::fs::File::create(dest).await.map_err(err)?;
    let mut hasher = Sha256::new();
    let mut got: u64 = 0;
    let mut last = -1.0;
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("{} 받기 중단: {}", name, e))?;
        hasher.update(&chunk);
        out.write_all(&chunk).await.map_err(err)?;
        got += chunk.len() as u64;
        if total > 0 {
            let p = got as f64 * 100.0 / total as f64;
            if p - last >= 1.0 {
                last = p;
                emit(app, name, "download", p);
            }
        }
    }
    out.flush().await.map_err(err)?;
    drop(out);
    let actual: String = hasher.finalize().iter().map(|b| format!("{:02x}", b)).collect();
    if actual != expected {
        let _ = tokio::fs::remove_file(dest).await;
        return Err(format!(
            "{} 파일이 공식 체크섬과 달라서 설치하지 않았어요. 잠시 후 다시 시도해 주세요.",
            name
        ));
    }
    Ok(())
}

/// Pulls the named files (matched by path suffix) out of a zip.
fn extract(zip_path: &Path, wanted: &[(String, PathBuf)]) -> Result<(), String> {
    let file = std::fs::File::open(zip_path).map_err(err)?;
    let mut zip = zip::ZipArchive::new(file).map_err(err)?;
    let mut found = 0;
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i).map_err(err)?;
        let name = entry.name().replace('\\', "/");
        for (suffix, dest) in wanted {
            if name.ends_with(suffix.as_str()) {
                let tmp = dest.with_extension("new");
                let mut out = std::fs::File::create(&tmp).map_err(err)?;
                std::io::copy(&mut entry, &mut out).map_err(err)?;
                drop(out);
                std::fs::rename(&tmp, dest).map_err(err)?;
                found += 1;
            }
        }
    }
    if found < wanted.len() {
        return Err("압축 파일에서 필요한 파일을 찾지 못했어요.".into());
    }
    Ok(())
}

async fn install_from_zip(
    app: &AppHandle,
    name: &str,
    url: &str,
    sums_url: &str,
    wanted: Vec<(String, PathBuf)>,
) -> Result<(), String> {
    let zip_path = bin_dir(app).join(format!("{}.zip", name));
    download_verified(app, name, url, sums_url, &zip_path).await?;
    emit(app, name, "extract", 100.0);
    let zp = zip_path.clone();
    tokio::task::spawn_blocking(move || extract(&zp, &wanted))
        .await
        .map_err(err)??;
    let _ = tokio::fs::remove_file(&zip_path).await;
    Ok(())
}

pub async fn install_tool(app: &AppHandle, name: &str, channel: &str) -> Result<(), String> {
    tokio::fs::create_dir_all(bin_dir(app)).await.map_err(err)?;
    match name {
        "yt-dlp" => {
            let (url, sums) = if channel == "nightly" {
                (YTDLP_NIGHTLY, YTDLP_NIGHTLY_SUMS)
            } else {
                (YTDLP_STABLE, YTDLP_STABLE_SUMS)
            };
            let tmp = bin_dir(app).join("yt-dlp.download");
            download_verified(app, name, url, sums, &tmp).await?;
            tokio::fs::rename(&tmp, tool_path(app, name)).await.map_err(err)?;
        }
        "ffmpeg" => {
            let wanted = vec![
                ("bin/ffmpeg.exe".to_string(), tool_path(app, "ffmpeg")),
                ("bin/ffprobe.exe".to_string(), tool_path(app, "ffprobe")),
            ];
            install_from_zip(app, name, FFMPEG_ZIP, FFMPEG_SUMS, wanted).await?;
        }
        "deno" => {
            let wanted = vec![("deno.exe".to_string(), tool_path(app, "deno"))];
            install_from_zip(app, name, DENO_ZIP, DENO_SUMS, wanted).await?;
        }
        _ => return Err(format!("unknown tool {}", name)),
    }
    emit(app, name, "done", 100.0);
    Ok(())
}

#[tauri::command]
pub async fn tools_status(app: AppHandle, channel: Option<String>, check_latest: bool) -> Vec<ToolInfo> {
    status(&app, channel.as_deref().unwrap_or("stable"), check_latest).await
}

/// First run: fetch whatever is missing.
#[tauri::command]
pub async fn install_tools(app: AppHandle, channel: Option<String>) -> Result<Vec<ToolInfo>, String> {
    let ch = channel.unwrap_or_else(|| "stable".into());
    for name in TOOLS {
        if tool_path(&app, name).exists() {
            emit(&app, name, "done", 100.0);
        } else {
            install_tool(&app, name, &ch).await?;
        }
    }
    Ok(status(&app, &ch, false).await)
}

/// The "update now" button and the once-a-day check at launch.
#[tauri::command]
pub async fn update_engine(app: AppHandle, channel: String) -> Result<Vec<ToolInfo>, String> {
    let channel = if channel == "nightly" { "nightly" } else { "stable" }.to_string();
    for name in TOOLS {
        if !tool_path(&app, name).exists() {
            install_tool(&app, name, &channel).await?;
        }
    }

    // yt-dlp updates itself; reinstall if that ever fails
    emit(&app, "yt-dlp", "update", 0.0);
    let yt = tool_path(&app, "yt-dlp");
    let ok = run_capture(&app, &yt, &["--update-to", &channel]).await.map(|r| r.0).unwrap_or(false);
    if !ok {
        install_tool(&app, "yt-dlp", &channel).await?;
    }
    emit(&app, "yt-dlp", "done", 100.0);

    let deno_local = local_version(&app, "deno").await;
    let deno_latest = github_latest("denoland/deno").await;
    if let (Some(a), Some(b)) = (&deno_local, &deno_latest) {
        if a != b {
            emit(&app, "deno", "update", 0.0);
            install_tool(&app, "deno", &channel).await?;
        }
    }
    emit(&app, "deno", "done", 100.0);

    if older_than(&tool_path(&app, "ffmpeg"), FFMPEG_MAX_AGE_DAYS) {
        install_tool(&app, "ffmpeg", &channel).await?;
    }
    emit(&app, "ffmpeg", "done", 100.0);

    Ok(status(&app, &channel, true).await)
}

/// Lets the user force a fresh ffmpeg (it is the one large download).
#[tauri::command]
pub async fn reinstall_tool(app: AppHandle, name: String, channel: Option<String>) -> Result<Vec<ToolInfo>, String> {
    let ch = channel.unwrap_or_else(|| "stable".into());
    install_tool(&app, &name, &ch).await?;
    Ok(status(&app, &ch, true).await)
}
