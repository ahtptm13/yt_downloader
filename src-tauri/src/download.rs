//! Reading a link (yt-dlp -J) and running downloads with live progress.

use crate::logic::{self, Job, Probe, Tracker};
use crate::tools::{bin_dir, command, err, tool_path};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};

/// Groups a download's whole process tree (yt-dlp and the ffmpeg it starts)
/// in a Windows Job Object, so cancelling stops all of it without shelling
/// out to taskkill, and closing the app never leaves orphans behind.
#[cfg(windows)]
mod proc_group {
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation, SetInformationJobObject,
        TerminateJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };

    pub struct Group(HANDLE);
    // the handle is only used through thread-safe Win32 calls
    unsafe impl Send for Group {}
    unsafe impl Sync for Group {}

    impl Group {
        pub fn new() -> Option<Group> {
            unsafe {
                let h = CreateJobObjectW(std::ptr::null(), std::ptr::null());
                if h.is_null() {
                    return None;
                }
                let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
                info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                SetInformationJobObject(
                    h,
                    JobObjectExtendedLimitInformation,
                    &info as *const _ as *const core::ffi::c_void,
                    std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                );
                Some(Group(h))
            }
        }

        pub fn add(&self, process: HANDLE) -> bool {
            unsafe { AssignProcessToJobObject(self.0, process) != 0 }
        }

        pub fn stop(&self) {
            unsafe {
                TerminateJobObject(self.0, 1);
            }
        }
    }

    impl Drop for Group {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
}

struct Running {
    #[cfg(windows)]
    group: Option<proc_group::Group>,
    #[allow(dead_code)]
    pid: u32,
}

#[derive(Default)]
pub struct Jobs {
    running: Mutex<HashMap<String, Arc<Running>>>,
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
    #[cfg(windows)]
    let running = {
        let group = proc_group::Group::new();
        if let (Some(g), Some(h)) = (&group, child.raw_handle()) {
            g.add(h as windows_sys::Win32::Foundation::HANDLE);
        }
        Running { group, pid }
    };
    #[cfg(not(windows))]
    let running = Running { pid };
    jobs.running.lock().unwrap().insert(job.id.clone(), Arc::new(running));

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
        // dropping the group closes it, which also ends any leftover ffmpeg
        state.running.lock().unwrap().remove(&id);
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
    let running = jobs.running.lock().unwrap().get(&id).cloned();
    let Some(running) = running else { return Ok(()) };
    jobs.cancelled.lock().unwrap().insert(id);
    #[cfg(windows)]
    {
        if let Some(g) = &running.group {
            g.stop();
        }
    }
    #[cfg(not(windows))]
    {
        let _ = tokio::process::Command::new("kill").arg(running.pid.to_string()).output().await;
    }
    Ok(())
}

pub fn any_running(app: &AppHandle) -> bool {
    !app.state::<Jobs>().running.lock().unwrap().is_empty()
}
