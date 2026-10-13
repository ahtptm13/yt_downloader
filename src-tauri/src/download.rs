//! Reading a link (yt-dlp -J) and running downloads with live progress.

use crate::logic::{self, Job, Probe, Tracker};
use crate::tools::{bin_dir, command, err, tool_path};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};

#[derive(Default)]
pub struct Jobs {
    pids: Mutex<HashMap<String, u32>>,
    cancelled: Mutex<HashSet<String>>,
}

#[derive(Serialize, Clone)]
struct JobProgress {
    id: String,
    #[serde(flatten)]
    p: logic::Progress,
}

#[derive(Serialize, Clone)]
struct JobDone {
    id: String,
    ok: bool,
    cancelled: bool,
    file: Option<String>,
    error: Option<String>,
    warning: Option<String>,
}

fn cookie_args(cookies: &Option<String>) -> Vec<String> {
    match cookies.as_deref() {
        Some(c) if !c.is_empty() => vec!["--cookies-from-browser".into(), c.into()],
        _ => vec![],
    }
}

fn deno_arg(app: &AppHandle) -> Option<String> {
    let p = tool_path(app, "deno");
    p.exists().then(|| p.to_string_lossy().into_owned())
}

#[tauri::command]
pub async fn probe(app: AppHandle, url: String, cookies: Option<String>) -> Result<Probe, String> {
    let url = url.trim().to_string();
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err("링크 형식이 아니에요.".into());
    }
    let mut args: Vec<String> = vec!["-J".into(), "--no-playlist".into(), "--flat-playlist".into(), "--no-warnings".into()];
    if let Some(d) = deno_arg(&app) {
        args.push("--js-runtimes".into());
        args.push(format!("deno:{}", d));
    }
    args.extend(cookie_args(&cookies));
    args.push("--".into());
    args.push(url.clone());
    let out = command(&app, &tool_path(&app, "yt-dlp"))
        .args(&args)
        .output()
        .await
        .map_err(|e| format!("다운로드 엔진을 실행하지 못했어요: {}", e))?;
    if !out.status.success() {
        return Err(logic::friendly_error(&String::from_utf8_lossy(&out.stderr)));
    }
    let info: serde_json::Value = serde_json::from_slice(&out.stdout).map_err(err)?;
    Ok(logic::build_probe(&info, &url))
}

async fn read_lines<R: AsyncRead + Unpin>(r: R, mut f: impl FnMut(String)) {
    let mut reader = BufReader::new(r);
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match reader.read_until(b'\n', &mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(_) => f(String::from_utf8_lossy(&buf).into_owned()),
        }
    }
}

#[tauri::command]
pub async fn start_download(app: AppHandle, jobs: State<'_, Jobs>, job: Job) -> Result<(), String> {
    std::fs::create_dir_all(&job.out_dir).map_err(|e| format!("저장 폴더를 만들 수 없어요: {}", e))?;
    let bin = bin_dir(&app).to_string_lossy().into_owned();
    let args = logic::build_args(&job, &bin, deno_arg(&app).as_deref());
    let mut child = command(&app, &tool_path(&app, "yt-dlp"))
        .args(&args)
        .spawn()
        .map_err(|e| format!("다운로드 엔진을 실행하지 못했어요: {}", e))?;
    let pid = child.id().unwrap_or(0);
    jobs.pids.lock().unwrap().insert(job.id.clone(), pid);

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let app2 = app.clone();
    tauri::async_runtime::spawn(async move {
        let id = job.id.clone();
        let err_task = tokio::spawn(async move {
            let mut text = String::new();
            if let Some(e) = stderr {
                read_lines(e, |l| text.push_str(&l)).await;
            }
            text
        });
        let mut tracker = Tracker::new(&job.kind);
        if let Some(out) = stdout {
            let app3 = app2.clone();
            let id3 = id.clone();
            read_lines(out, |line| {
                if let Some(p) = tracker.feed(&line) {
                    let _ = app3.emit("job-progress", JobProgress { id: id3.clone(), p });
                }
            })
            .await;
        }
        let exited_ok = child.wait().await.map(|s| s.success()).unwrap_or(false);
        let stderr_text = err_task.await.unwrap_or_default();
        let success = exited_ok && !logic::has_hard_error(&stderr_text);
        let state = app2.state::<Jobs>();
        state.pids.lock().unwrap().remove(&id);
        let cancelled = state.cancelled.lock().unwrap().remove(&id);
        if cancelled {
            cleanup_partials(&job);
        }
        let _ = app2.emit(
            "job-done",
            JobDone {
                id,
                ok: success && !cancelled,
                cancelled,
                file: tracker.file.clone(),
                error: (!success && !cancelled).then(|| logic::friendly_error(&stderr_text)),
                warning: logic::subtitle_warning(&stderr_text),
            },
        );
    });
    Ok(())
}

/// Removes the half-finished pieces a cancelled download leaves behind.
fn cleanup_partials(job: &Job) {
    let Some(vid) = job.video_id.as_deref().filter(|v| !v.is_empty()) else { return };
    let tag = format!("[{}]", vid);
    let Ok(rd) = std::fs::read_dir(&job.out_dir) else { return };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if name.contains(&tag) && (name.ends_with(".part") || name.ends_with(".ytdl") || name.contains(".part-Frag")) {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

#[tauri::command]
pub async fn cancel_download(jobs: State<'_, Jobs>, id: String) -> Result<(), String> {
    let pid = jobs.pids.lock().unwrap().get(&id).copied();
    let Some(pid) = pid else { return Ok(()) };
    jobs.cancelled.lock().unwrap().insert(id);
    #[cfg(windows)]
    {
        // /T also stops the ffmpeg that yt-dlp may have started
        let mut c = tokio::process::Command::new("taskkill");
        c.args(["/PID", &pid.to_string(), "/T", "/F"]);
        c.creation_flags(0x0800_0000);
        let _ = c.output().await;
    }
    #[cfg(not(windows))]
    {
        let _ = tokio::process::Command::new("kill").arg(pid.to_string()).output().await;
    }
    Ok(())
}

pub fn any_running(app: &AppHandle) -> bool {
    !app.state::<Jobs>().pids.lock().unwrap().is_empty()
}
