//! Pure logic with no Tauri dependency: reading yt-dlp's JSON, building the
//! command line, and turning yt-dlp's output lines into progress updates.
//! Kept separate so it can be unit-tested on any machine.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeSet, HashSet};

#[derive(Serialize, Clone, Debug)]
pub struct Quality {
    pub id: String,
    /// resolution group shown in the left list: "2160p", "1080p", "음원만"
    pub group: String,
    /// codec shown in the right list: "AV1", "VP9", "H.264", "MP3"...
    pub codec: String,
    pub label: String,
    pub detail: String,
    /// "best" | "edit" | "" — used for the row badge and default selection
    pub role: String,
    pub size: Option<u64>,
    /// value passed to yt-dlp -f
    pub format: String,
    /// "video" | "mp3" | "m4a" | "opus"
    pub kind: String,
    /// "auto" keeps the original streams (WebM when AV1/VP9 + Opus), "mp4" forces MP4
    pub container: String,
    /// what the file will be: "WebM", "MP4", "MP3"...
    pub ext: String,
}

#[derive(Serialize, Clone, Debug)]
pub struct Sub {
    pub code: String,
    pub name: String,
    pub auto: bool,
}

#[derive(Serialize, Clone, Debug)]
pub struct Probe {
    pub kind: String,
    pub id: String,
    pub url: String,
    pub title: String,
    pub channel: String,
    pub duration: Option<f64>,
    pub thumbnail: Option<String>,
    pub count: Option<usize>,
    pub qualities: Vec<Quality>,
    pub subs: Vec<Sub>,
}

#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Job {
    pub id: String,
    pub url: String,
    pub video_id: Option<String>,
    pub format: String,
    pub kind: String,
    #[serde(default)]
    pub container: String,
    pub sub_langs: Vec<String>,
    pub write_subs: bool,
    pub write_auto_subs: bool,
    pub sub_format: String,
    pub thumbnail: bool,
    pub thumb_format: String,
    pub description: bool,
    pub out_dir: String,
    pub name_tpl: String,
    pub cookies: Option<String>,
    pub playlist: bool,
}

fn s(v: &Value) -> &str {
    v.as_str().unwrap_or("")
}

fn num(v: &Value) -> Option<f64> {
    v.as_f64()
}

fn fsize(f: &Value) -> Option<u64> {
    num(&f["filesize"])
        .or_else(|| num(&f["filesize_approx"]))
        .map(|x| x as u64)
}

fn vcodec(f: &Value) -> &str {
    match f["vcodec"].as_str() {
        Some(v) => v,
        None => "none",
    }
}

fn acodec(f: &Value) -> &str {
    match f["acodec"].as_str() {
        Some(v) => v,
        None => "none",
    }
}

fn codec_name(vc: &str) -> &'static str {
    if vc.starts_with("av01") {
        "AV1"
    } else if vc.starts_with("vp09") || vc.starts_with("vp9") {
        "VP9"
    } else if vc.starts_with("avc") {
        "H.264"
    } else if vc.starts_with("hev") || vc.starts_with("hvc") {
        "HEVC"
    } else {
        "영상"
    }
}

fn is_sdr(f: &Value) -> bool {
    matches!(f["dynamic_range"].as_str(), None | Some("SDR"))
}

#[allow(clippy::too_many_arguments)]
fn q(id: &str, group: &str, codec: &str, detail: &str, role: &str, size: Option<u64>, format: &str, kind: &str, container: &str, ext: &str) -> Quality {
    Quality {
        id: id.into(),
        group: group.into(),
        codec: codec.into(),
        label: group.into(),
        detail: detail.into(),
        role: role.into(),
        size,
        format: format.into(),
        kind: kind.into(),
        container: container.into(),
        ext: ext.into(),
    }
}

pub const AUDIO_GROUP: &str = "음원만";

pub fn audio_rows(best_audio: Option<u64>, best_m4a: Option<u64>, best_is_opus: bool) -> Vec<Quality> {
    let mut v = Vec::new();
    if best_is_opus {
        v.push(q("opus", AUDIO_GROUP, "Opus", "최고 음질 원본 · 재인코딩 없음", "", best_audio, "ba/b", "opus", "auto", "Opus"));
    }
    v.push(q("m4a", AUDIO_GROUP, "AAC", "M4A · 편집 호환", "", best_m4a, "ba[ext=m4a]/ba/b", "m4a", "auto", "M4A"));
    v.push(q("mp3", AUDIO_GROUP, "MP3", "MP3로 변환", "", best_audio, "ba/b", "mp3", "auto", "MP3"));
    v
}

/// Generic choices when individual formats are unknown (playlists).
pub fn presets() -> Vec<Quality> {
    let mut v = Vec::new();
    for (h, label) in [(0u64, "최고"), (2160, "2160p"), (1440, "1440p"), (1080, "1080p"), (720, "720p")] {
        let cap = if h == 0 { String::new() } else { format!("[height<={}]", h) };
        let best_role = if h == 0 { "best" } else { "" };
        v.push(q(
            &format!("p{}-best", h),
            label,
            "원본 코덱",
            "영상마다 가장 좋은 코덱 (AV1·VP9)",
            best_role,
            None,
            &format!("bv*{c}+ba/b{c}/b", c = cap),
            "video",
            "auto",
            "WebM",
        ));
        if h != 0 && h <= 1080 {
            v.push(q(
                &format!("p{}-h264", h),
                label,
                "H.264",
                "편집 호환",
                if h == 1080 { "edit" } else { "" },
                None,
                &format!("bv*[vcodec^=avc1]{c}+ba[ext=m4a]/b{c}/b", c = cap),
                "video",
                "mp4",
                "MP4",
            ));
        }
    }
    v.extend(audio_rows(None, None, true));
    v
}

fn pick_max<'a>(items: &[&'a Value], key: impl Fn(&Value) -> (u8, u8, i64, i64)) -> Option<&'a Value> {
    items.iter().copied().max_by_key(|f| key(f))
}

fn codec_slug(codec: &str) -> &'static str {
    match codec {
        "AV1" => "av1",
        "VP9" => "vp9",
        "HEVC" => "hevc",
        "H.264" => "h264",
        _ => "other",
    }
}

/// Every resolution × codec (× HDR) the video offers, best first, plus audio-only rows.
pub fn qualities_from(info: &Value) -> Vec<Quality> {
    let formats = match info["formats"].as_array() {
        Some(f) if !f.is_empty() => f,
        _ => return presets(),
    };
    let is_video = |f: &Value| {
        vcodec(f) != "none"
            && f["height"].as_u64().unwrap_or(0) > 0
            && s(&f["protocol"]) != "mhtml"
            && !s(&f["format_id"]).is_empty()
    };
    let audios: Vec<&Value> = formats
        .iter()
        .filter(|f| vcodec(f) == "none" && acodec(f) != "none")
        .collect();
    let abr = |f: &Value| (num(&f["abr"]).or_else(|| num(&f["tbr"])).unwrap_or(0.0) * 10.0) as i64;
    let best_audio = pick_max(&audios, |f| (0, 0, abr(f), 0));
    let m4as: Vec<&Value> = audios
        .iter()
        .copied()
        .filter(|f| s(&f["ext"]) == "m4a" || acodec(f).starts_with("mp4a"))
        .collect();
    let best_m4a = pick_max(&m4as, |f| (0, 0, abr(f), 0));
    let best_is_opus = best_audio.map(|a| acodec(a).starts_with("opus")).unwrap_or(false);

    let mut vids: Vec<&Value> = formats
        .iter()
        .filter(|f| is_video(f) && acodec(f) == "none")
        .collect();
    let separate = !vids.is_empty();
    if !separate {
        vids = formats.iter().filter(|f| is_video(f)).collect();
    }
    if vids.is_empty() {
        let mut v = audio_rows(best_audio.and_then(fsize), best_m4a.and_then(fsize), best_is_opus);
        if let Some(first) = v.first_mut() {
            first.role = "best".into();
        }
        return v;
    }

    let heights: BTreeSet<u64> = vids.iter().filter_map(|f| f["height"].as_u64()).collect();
    let max_h = *heights.iter().next_back().unwrap_or(&0);
    let mut rows: Vec<Quality> = Vec::new();
    for &h in heights.iter().rev() {
        if h < 360 && max_h >= 360 {
            continue;
        }
        let group = format!("{}p", h);
        // one row per codec and dynamic range at this height: the highest fps, then bitrate
        let mut combos: Vec<(&'static str, bool)> = Vec::new();
        for f in vids.iter().filter(|f| f["height"].as_u64() == Some(h)) {
            let c = (codec_name(vcodec(f)), is_sdr(f));
            if !combos.contains(&c) {
                combos.push(c);
            }
        }
        combos.sort_by_key(|(c, sdr)| (std::cmp::Reverse(*sdr), std::cmp::Reverse(codec_rank_name(c))));
        for (codec, sdr) in combos {
            let cands: Vec<&Value> = vids
                .iter()
                .copied()
                .filter(|f| f["height"].as_u64() == Some(h) && codec_name(vcodec(f)) == codec && is_sdr(f) == sdr)
                .collect();
            let Some(v) = pick_max(&cands, |f| {
                (0, 0, num(&f["fps"]).unwrap_or(0.0) as i64, (num(&f["tbr"]).unwrap_or(0.0) * 10.0) as i64)
            }) else {
                continue;
            };
            let mp4_codec = codec == "H.264" || codec == "HEVC";
            // AV1/VP9 keep the best audio (Opus) in WebM; H.264 pairs with AAC in MP4
            let audio = if mp4_codec { best_m4a.or(best_audio) } else { best_audio };
            let vid = s(&v["format_id"]);
            let (main, size) = if !separate {
                (vid.to_string(), fsize(v))
            } else if let Some(a) = audio {
                (format!("{}+{}", vid, s(&a["format_id"])), fsize(v).map(|x| x + fsize(a).unwrap_or(0)))
            } else {
                (format!("{}+ba", vid), fsize(v))
            };
            let mut detail = match num(&v["fps"]).map(|x| x.round() as i64) {
                Some(f) if f > 0 => format!("{}fps", f),
                _ => String::new(),
            };
            if !sdr {
                detail.push_str(if detail.is_empty() { "HDR" } else { " · HDR" });
            }
            let ext = if mp4_codec || audio.map(|a| !acodec(a).starts_with("opus")).unwrap_or(false) { "MP4" } else { "WebM" };
            rows.push(q(
                &format!("v{}-{}{}", h, codec_slug(codec), if sdr { "" } else { "-hdr" }),
                &group,
                codec,
                &detail,
                "",
                size,
                &format!("{}/bv*[height<={h}]+ba/b[height<={h}]/b", main, h = h),
                "video",
                if mp4_codec { "mp4" } else { "auto" },
                ext,
            ));
        }
    }
    if let Some(first) = rows.first_mut() {
        first.role = "best".into();
    }
    // highest H.264 up to 1080p: opens in any editor
    if let Some(edit) = rows
        .iter_mut()
        .filter(|r| r.role.is_empty() && r.codec == "H.264" && !r.id.ends_with("-hdr"))
        .find(|r| r.group.trim_end_matches('p').parse::<u64>().map(|h| h <= 1080).unwrap_or(false))
    {
        edit.role = "edit".into();
    }
    rows.extend(audio_rows(best_audio.and_then(fsize), best_m4a.and_then(fsize), best_is_opus));
    rows
}

fn codec_rank_name(c: &str) -> u8 {
    match c {
        "AV1" => 4,
        "VP9" => 3,
        "HEVC" => 2,
        "H.264" => 1,
        _ => 0,
    }
}

fn sub_name(list: &Value, code: &str) -> String {
    list.as_array()
        .and_then(|a| a.first())
        .and_then(|e| e["name"].as_str())
        .map(|n| n.to_string())
        .unwrap_or_else(|| code.to_string())
}

pub fn subs_from(info: &Value) -> Vec<Sub> {
    let mut out = Vec::new();
    let mut manual: HashSet<String> = HashSet::new();
    if let Some(m) = info["subtitles"].as_object() {
        for (code, list) in m {
            if code == "live_chat" {
                continue;
            }
            manual.insert(code.clone());
            out.push(Sub { code: code.clone(), name: sub_name(list, code), auto: false });
        }
    }
    if let Some(m) = info["automatic_captions"].as_object() {
        for (code, list) in m {
            let base = code.split('-').next().unwrap_or(code);
            if manual.contains(code) || manual.contains(base) {
                continue;
            }
            out.push(Sub { code: code.clone(), name: sub_name(list, code), auto: true });
        }
    }
    out
}

pub fn build_probe(info: &Value, url: &str) -> Probe {
    let first = |keys: &[&str]| {
        keys.iter()
            .map(|k| s(&info[*k]))
            .find(|v| !v.is_empty())
            .unwrap_or("")
            .to_string()
    };
    let thumbnail = info["thumbnail"]
        .as_str()
        .map(String::from)
        .or_else(|| {
            info["thumbnails"]
                .as_array()
                .and_then(|a| a.last())
                .and_then(|t| t["url"].as_str())
                .map(String::from)
        })
        .or_else(|| {
            info["entries"]
                .as_array()
                .and_then(|e| e.first())
                .and_then(|e| e["thumbnails"].as_array())
                .and_then(|a| a.last())
                .and_then(|t| t["url"].as_str())
                .map(String::from)
        });
    let is_playlist = s(&info["_type"]) == "playlist";
    Probe {
        kind: if is_playlist { "playlist" } else { "video" }.into(),
        id: s(&info["id"]).to_string(),
        url: url.to_string(),
        title: first(&["title", "fulltitle", "id"]),
        channel: first(&["channel", "uploader", "playlist_uploader", "extractor_key"]),
        duration: num(&info["duration"]),
        thumbnail,
        count: if is_playlist {
            info["entries"].as_array().map(|a| a.len())
        } else {
            None
        },
        qualities: if is_playlist { presets() } else { qualities_from(info) },
        subs: if is_playlist { Vec::new() } else { subs_from(info) },
    }
}

pub const PROGRESS_TEMPLATE: &str = "download:GRB|%(progress.status)s|%(progress.downloaded_bytes)s|%(progress.total_bytes,progress.total_bytes_estimate)s|%(progress.speed)s|%(progress.eta)s|%(info.vcodec)s";

pub fn output_template(name_tpl: &str, playlist: bool) -> String {
    let base = match name_tpl {
        "title" => "%(title)s.%(ext)s",
        "date_title" => "%(upload_date>%Y-%m-%d)s - %(title)s.%(ext)s",
        _ => "%(title)s [%(id)s].%(ext)s",
    };
    if playlist {
        format!("%(playlist_title)s/%(playlist_index)03d - {}", base)
    } else {
        base.to_string()
    }
}

pub fn build_args(j: &Job, bin_dir: &str, deno_path: Option<&str>) -> Vec<String> {
    let mut a: Vec<String> = Vec::new();
    let mut push = |x: &str| a.push(x.to_string());
    // --ignore-errors turns a failed subtitle into a warning instead of
    // failing the whole video; real failures still exit non-zero
    for x in ["--newline", "--progress", "--no-mtime", "--no-colors", "--ignore-errors"] {
        push(x);
    }
    push("--progress-template");
    push(PROGRESS_TEMPLATE);
    push("--ffmpeg-location");
    push(bin_dir);
    if let Some(d) = deno_path {
        push("--js-runtimes");
        push(&format!("deno:{}", d));
    }
    push("-P");
    push(&j.out_dir);
    push("-o");
    push(&output_template(&j.name_tpl, j.playlist));
    push(if j.playlist { "--yes-playlist" } else { "--no-playlist" });
    push("-f");
    push(&j.format);
    match j.kind.as_str() {
        "mp3" => {
            for x in ["-x", "--audio-format", "mp3", "--audio-quality", "0"] {
                push(x);
            }
        }
        "m4a" => {
            for x in ["-x", "--audio-format", "m4a"] {
                push(x);
            }
        }
        "opus" => {
            // keep the original audio stream, no re-encoding
            for x in ["-x", "--audio-format", "best"] {
                push(x);
            }
        }
        _ => {
            push("--merge-output-format");
            // first container that can hold the streams without re-encoding
            push(if j.container == "mp4" { "mp4" } else { "webm/mp4/mkv" });
        }
    }
    if !j.sub_langs.is_empty() && (j.write_subs || j.write_auto_subs) {
        if j.write_subs {
            push("--write-subs");
        }
        if j.write_auto_subs {
            push("--write-auto-subs");
        }
        push("--sub-langs");
        push(&j.sub_langs.join(","));
        // YouTube rate-limits caption requests fired back to back
        push("--sleep-subtitles");
        push("1");
        let embed = j.sub_format == "embed" && j.kind == "video";
        if embed {
            push("--embed-subs");
        } else if j.sub_format == "vtt" {
            push("--sub-format");
            push("vtt/best");
        } else {
            push("--convert-subs");
            push("srt");
        }
    }
    if j.thumbnail {
        push("--write-thumbnail");
        match j.thumb_format.as_str() {
            "png" => {
                push("--convert-thumbnails");
                push("png");
            }
            "orig" => {}
            _ => {
                push("--convert-thumbnails");
                push("jpg");
            }
        }
    }
    if j.description {
        push("--write-description");
    }
    if let Some(c) = j.cookies.as_deref().filter(|c| !c.is_empty()) {
        push("--cookies-from-browser");
        push(c);
    }
    // "--" so a link can never be read as an option
    push("--");
    push(&j.url);
    a
}

#[derive(Serialize, Clone, Debug, Default)]
pub struct Progress {
    pub stage: String,
    pub percent: Option<f64>,
    pub downloaded: Option<u64>,
    pub total: Option<u64>,
    pub speed: Option<f64>,
    pub eta: Option<u64>,
    pub item: Option<u64>,
    pub items: Option<u64>,
}

const SIDE_EXT: &[&str] = &[
    "vtt", "srt", "ass", "lrc", "json3", "srv1", "srv2", "srv3", "ttml", "jpg", "jpeg", "png",
    "webp", "description", "json",
];

fn is_side_file(path: &str) -> bool {
    let ext = path.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    SIDE_EXT.contains(&ext.as_str())
}

/// Follows yt-dlp's stdout and reports where the job is.
pub struct Tracker {
    kind: String,
    pub stage: String,
    pub file: Option<String>,
    item: Option<u64>,
    items: Option<u64>,
}

impl Tracker {
    pub fn new(kind: &str) -> Self {
        Tracker { kind: kind.to_string(), stage: "info".into(), file: None, item: None, items: None }
    }

    fn snapshot(&self) -> Progress {
        Progress { stage: self.stage.clone(), item: self.item, items: self.items, ..Default::default() }
    }

    pub fn feed(&mut self, raw: &str) -> Option<Progress> {
        let line = raw.trim_end_matches(['\r', '\n']);
        if let Some(rest) = line.strip_prefix("GRB|") {
            let p: Vec<&str> = rest.split('|').collect();
            let n = |i: usize| p.get(i).and_then(|x| x.parse::<f64>().ok());
            let downloaded = n(1).map(|x| x as u64);
            let total = n(2).map(|x| x as u64);
            let vc = p.get(5).copied().unwrap_or("NA");
            // vcodec is "none" for an audio-only stream, "NA" when unknown
            self.stage = if self.kind == "video" && vc != "none" { "video".into() } else { "audio".into() };
            let percent = match (downloaded, total) {
                (Some(d), Some(t)) if t > 0 => Some((d as f64 * 100.0 / t as f64).min(100.0)),
                _ => None,
            };
            return Some(Progress {
                stage: self.stage.clone(),
                percent,
                downloaded,
                total,
                speed: n(3),
                eta: n(4).map(|x| x as u64),
                item: self.item,
                items: self.items,
            });
        }
        if let Some(rest) = line.strip_prefix("[download] Downloading item ") {
            // "3 of 20"
            let mut it = rest.split_whitespace();
            self.item = it.next().and_then(|x| x.parse().ok());
            self.items = it.nth(1).and_then(|x| x.parse().ok());
            return Some(self.snapshot());
        }
        if let Some(rest) = line.strip_prefix("[Merger] Merging formats into \"") {
            self.file = Some(rest.trim_end_matches('"').to_string());
            self.stage = "finish".into();
            return Some(self.snapshot());
        }
        if let Some(rest) = line.strip_prefix("[ExtractAudio] Destination: ") {
            self.file = Some(rest.to_string());
            self.stage = "finish".into();
            return Some(self.snapshot());
        }
        if let Some(rest) = line.strip_prefix("[download] Destination: ") {
            if !is_side_file(rest) {
                self.file = Some(rest.to_string());
            }
            return None;
        }
        if let Some(rest) = line.strip_prefix("[download] ") {
            if let Some(path) = rest.strip_suffix(" has already been downloaded") {
                if !is_side_file(path) {
                    self.file = Some(path.to_string());
                }
                return None;
            }
        }
        const FINISH: &[&str] = &[
            "[VideoRemuxer]", "[VideoConvertor]", "[EmbedSubtitle]", "[Metadata]", "[FixupM4a]",
            "[FixupM3u8]", "[FixupDuplicateMoov]", "[SubtitlesConvertor]", "[ThumbnailsConvertor]",
        ];
        if FINISH.iter().any(|t| line.starts_with(t)) && self.stage != "info" {
            self.stage = "finish".into();
            return Some(self.snapshot());
        }
        None
    }
}

/// Turns yt-dlp's stderr into one short message in Korean.
pub fn friendly_error(stderr: &str) -> String {
    let last = stderr
        .lines()
        .filter(|l| l.contains("ERROR"))
        .last()
        .or_else(|| stderr.lines().filter(|l| !l.trim().is_empty()).last())
        .unwrap_or("")
        .trim();
    let msg = last.trim_start_matches("ERROR:").trim();
    let low = msg.to_lowercase();
    let hint = if low.contains("sign in to confirm your age") || low.contains("age-restricted") {
        "연령 제한 영상이에요. 설정에서 로그인 쿠키를 지정하면 받을 수 있어요."
    } else if low.contains("sign in to confirm") || low.contains("not a bot") {
        "유튜브가 로그인을 요구해요. 엔진을 업데이트하거나 로그인 쿠키를 지정해 보세요."
    } else if low.contains("members-only") || low.contains("join this channel") {
        "멤버십 전용 영상이에요. 로그인 쿠키가 필요해요."
    } else if low.contains("private video") {
        "비공개 영상이에요."
    } else if low.contains("video unavailable") || low.contains("not available") {
        "볼 수 없는 영상이에요."
    } else if low.contains("unsupported url") {
        "지원하지 않는 링크예요."
    } else if low.contains("requested format is not available") {
        "선택한 화질을 받을 수 없어요. 다른 화질을 골라 주세요."
    } else if low.contains("http error 429") || low.contains("too many requests") {
        "요청이 너무 많아 잠시 막혔어요. 잠시 후 다시 시도해 주세요."
    } else if low.contains("could not copy") && low.contains("cookie") {
        "브라우저 쿠키를 읽지 못했어요. 브라우저를 닫고 다시 시도해 주세요."
    } else if low.contains("unable to download") || low.contains("unable to extract") || low.contains("nsig") || low.contains("signature") {
        "유튜브 쪽 변경으로 실패했어요. 엔진을 업데이트한 뒤 다시 시도해 주세요."
    } else {
        ""
    };
    match (hint.is_empty(), msg.is_empty()) {
        (true, true) => "알 수 없는 오류가 났어요.".into(),
        (true, false) => msg.to_string(),
        (false, true) => hint.into(),
        (false, false) => format!("{}\n({})", hint, msg),
    }
}

/// With --ignore-errors a broken merge or conversion still exits 0,
/// so any ERROR line other than a skipped subtitle means the job failed.
pub fn has_hard_error(stderr: &str) -> bool {
    stderr.lines().any(|l| l.trim_start().starts_with("ERROR:"))
}

/// Subtitles that could not be fetched, as one Korean line, or None.
pub fn subtitle_warning(stderr: &str) -> Option<String> {
    let mut langs: Vec<String> = Vec::new();
    let mut limited = false;
    for l in stderr.lines() {
        if let Some(i) = l.find("Unable to download video subtitles for '") {
            let rest = &l[i + "Unable to download video subtitles for '".len()..];
            if let Some(end) = rest.find('\'') {
                let lang = rest[..end].to_string();
                if !langs.contains(&lang) {
                    langs.push(lang);
                }
            }
            if l.contains("429") {
                limited = true;
            }
        }
    }
    if langs.is_empty() {
        return None;
    }
    let why = if limited {
        "유튜브가 자막 요청을 잠시 막았어요. 자동 번역 자막에서 자주 생겨요"
    } else {
        "자막 서버에서 받지 못했어요"
    };
    Some(format!("자막 {}개를 받지 못했어요 ({}) · {}", langs.len(), langs.join(", "), why))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample() -> Value {
        json!({
            "id": "abc123", "title": "샘플 영상", "channel": "채널", "duration": 754.0,
            "thumbnail": "https://i.ytimg.com/vi/abc123/maxresdefault.jpg",
            "formats": [
                {"format_id": "sb0", "vcodec": "none", "acodec": "none", "protocol": "mhtml", "ext": "mhtml"},
                {"format_id": "140", "vcodec": "none", "acodec": "mp4a.40.2", "ext": "m4a", "abr": 129.5, "filesize": 10_000_000},
                {"format_id": "251", "vcodec": "none", "acodec": "opus", "ext": "webm", "abr": 140.0, "filesize": 11_000_000},
                {"format_id": "137", "vcodec": "avc1.640028", "acodec": "none", "height": 1080, "fps": 30, "tbr": 4000.0, "filesize": 300_000_000},
                {"format_id": "248", "vcodec": "vp9", "acodec": "none", "height": 1080, "fps": 30, "tbr": 2500.0, "filesize": 200_000_000},
                {"format_id": "399", "vcodec": "av01.0.08M.08", "acodec": "none", "height": 1080, "fps": 30, "tbr": 2000.0},
                {"format_id": "401", "vcodec": "av01.0.12M.08", "acodec": "none", "height": 2160, "fps": 60, "tbr": 17000.0, "filesize_approx": 1_500_000_000},
                {"format_id": "701", "vcodec": "av01.0.13M.10", "acodec": "none", "height": 2160, "fps": 60, "tbr": 20000.0, "dynamic_range": "HDR10"},
                {"format_id": "136", "vcodec": "avc1.4d401f", "acodec": "none", "height": 720, "fps": 30, "tbr": 1500.0},
                {"format_id": "160", "vcodec": "avc1.4d400c", "acodec": "none", "height": 144, "fps": 30},
                {"format_id": "18", "vcodec": "avc1.42001E", "acodec": "mp4a.40.2", "height": 360, "fps": 30}
            ],
            "subtitles": {"en": [{"ext": "vtt", "name": "English"}], "live_chat": [{}]},
            "automatic_captions": {
                "en": [{"name": "English"}], "en-orig": [{"name": "English (Original)"}], "ko": [{"name": "Korean"}]
            }
        })
    }

    #[test]
    fn every_resolution_and_codec_is_offered() {
        let q = qualities_from(&sample());
        let ids: Vec<&str> = q.iter().map(|x| x.id.as_str()).collect();
        // 144p is dropped, the muxed 360p stream is ignored when DASH exists; SDR before HDR, AV1 > VP9 > H.264
        assert_eq!(
            ids,
            ["v2160-av1", "v2160-av1-hdr", "v1080-av1", "v1080-vp9", "v1080-h264", "v720-h264", "opus", "m4a", "mp3"]
        );
        let by = |id: &str| q.iter().find(|x| x.id == id).unwrap();
        let best = by("v2160-av1");
        assert_eq!((best.role.as_str(), best.group.as_str(), best.codec.as_str()), ("best", "2160p", "AV1"));
        assert!(best.format.starts_with("401+251/"), "AV1 with Opus: {}", best.format);
        assert_eq!(best.size, Some(1_511_000_000));
        assert_eq!((best.container.as_str(), best.ext.as_str()), ("auto", "WebM"));
        assert_eq!(by("v2160-av1-hdr").detail, "60fps · HDR");
        assert!(by("v1080-vp9").format.starts_with("248+251/"));
        assert_eq!(by("v1080-vp9").size, Some(211_000_000));
        let edit = by("v1080-h264");
        assert_eq!(edit.role, "edit");
        assert!(edit.format.starts_with("137+140/"), "H.264 pairs with AAC");
        assert_eq!((edit.container.as_str(), edit.ext.as_str()), ("mp4", "MP4"));
        assert_eq!(by("v720-h264").role, "");
        assert_eq!(by("opus").group, AUDIO_GROUP);
        assert_eq!(by("mp3").size, Some(11_000_000));
        assert_eq!(by("m4a").size, Some(10_000_000));
    }

    #[test]
    fn subs_skip_live_chat_and_manual_duplicates() {
        let s = subs_from(&sample());
        let codes: Vec<(&str, bool)> = s.iter().map(|x| (x.code.as_str(), x.auto)).collect();
        assert!(codes.contains(&("en", false)));
        assert!(codes.contains(&("ko", true)));
        assert!(!codes.iter().any(|c| c.0 == "live_chat"));
        assert!(!codes.contains(&("en", true)));
        assert!(!codes.contains(&("en-orig", true)), "manual English hides auto English");
    }

    #[test]
    fn playlist_uses_presets() {
        let p = build_probe(&json!({"_type": "playlist", "id": "PL1", "title": "목록", "entries": [{}, {}, {}]}), "https://x");
        assert_eq!(p.kind, "playlist");
        assert_eq!(p.count, Some(3));
        assert_eq!(p.qualities[0].role, "best");
        assert!(p.qualities.iter().any(|x| x.role == "edit" && x.container == "mp4"));
    }

    #[test]
    fn args_are_safe_and_complete() {
        let j = Job {
            id: "1".into(), url: "-https://evil".into(), video_id: None, format: "137+140".into(),
            kind: "video".into(), container: "auto".into(), sub_langs: vec!["ko".into(), "en".into()], write_subs: true,
            write_auto_subs: true, sub_format: "srt".into(), thumbnail: true, thumb_format: "jpg".into(),
            description: false, out_dir: "C:/out".into(), name_tpl: "title_id".into(), cookies: None, playlist: false,
        };
        let a = build_args(&j, "C:/bin", Some("C:/bin/deno.exe"));
        let dash = a.iter().position(|x| x == "--").unwrap();
        assert_eq!(dash, a.len() - 2, "url comes right after --");
        for want in ["--write-subs", "--write-auto-subs", "--convert-subs", "--write-thumbnail", "--no-playlist", "--merge-output-format", "--ignore-errors", "--sleep-subtitles"] {
            assert!(a.contains(&want.to_string()), "missing {}", want);
        }
        assert!(a.contains(&"deno:C:/bin/deno.exe".to_string()));
        assert!(a.contains(&"ko,en".to_string()));
        assert!(a.contains(&"webm/mp4/mkv".to_string()));
        let mp4 = build_args(&Job { container: "mp4".into(), ..j }, "C:/bin", None);
        let i = mp4.iter().position(|x| x == "--merge-output-format").unwrap();
        assert_eq!(mp4[i + 1], "mp4");
    }

    #[test]
    fn tracker_follows_real_output() {
        let mut t = Tracker::new("video");
        assert!(t.feed("[youtube] abc: Downloading webpage").is_none());
        assert!(t.feed("[download] Destination: C:/out/샘플 [abc].ko.vtt").is_none());
        assert!(t.file.is_none(), "subtitle files are not the result");
        t.feed("[download] Destination: C:/out/샘플 [abc].f137.mp4");
        let p = t.feed("GRB|downloading|1024|3000000|1135733.96|2|avc1.640028\n").unwrap();
        assert_eq!(p.stage, "video");
        assert_eq!(p.eta, Some(2));
        assert!((p.percent.unwrap() - 0.0341).abs() < 0.01);
        assert_eq!(t.feed("GRB|downloading|10|100|NA|NA|none").unwrap().stage, "audio");
        assert_eq!(t.feed("GRB|downloading|10|100|NA|NA|NA").unwrap().stage, "video");
        let m = t.feed("[Merger] Merging formats into \"C:/out/샘플 [abc].mp4\"").unwrap();
        assert_eq!(m.stage, "finish");
        assert_eq!(t.file.as_deref(), Some("C:/out/샘플 [abc].mp4"));

        let mut a = Tracker::new("mp3");
        assert_eq!(a.feed("GRB|downloading|1|2|NA|NA|NA").unwrap().stage, "audio");
        a.feed("[ExtractAudio] Destination: C:/out/x.mp3");
        assert_eq!(a.file.as_deref(), Some("C:/out/x.mp3"));

        let mut pl = Tracker::new("video");
        let p = pl.feed("[download] Downloading item 3 of 20").unwrap();
        assert_eq!((p.item, p.items), (Some(3), Some(20)));
    }

    #[test]
    fn subtitle_failures_are_warnings_not_errors() {
        let err = "WARNING: Unable to download video subtitles for 'ko': HTTP Error 429: Too Many Requests\n";
        assert!(!has_hard_error(err));
        let w = subtitle_warning(err).unwrap();
        assert!(w.contains("ko") && w.contains("막았어요"), "{}", w);
        assert!(subtitle_warning("WARNING: something else").is_none());
        assert!(has_hard_error("ERROR: Postprocessing: Conversion failed!"));
    }

    #[test]
    fn errors_get_a_korean_hint() {
        let e = friendly_error("WARNING: x\nERROR: [youtube] abc: Sign in to confirm you're not a bot\n");
        assert!(e.contains("로그인"));
        assert_eq!(friendly_error(""), "알 수 없는 오류가 났어요.");
    }
}
