// Copyright (C) 2026 Mutantcat Working Group
// SPDX-License-Identifier: GPL-3.0-only
//! 视频 -> 帧序列：把一段视频读成几张静帧，交给量化器落进文档。
//!
//! 两条来源：
//! 1. 一个真正的视频文件，本机装有 ffmpeg/ffprobe 时抽帧。这是「读视频的模型」之外
//!    更实际的一条路——模型不一定吃视频，但 ffmpeg 一定在了。
//! 2. 一个装帧图的目录，直接被当作已有序列读，完全不碰外部进程。
//!
//! 进程调用全部收在 `spawn_blocking` 里：本 crate 的 tokio 没有开 process 特性，
//! 用 `std::process::Command` 阻塞一会儿比为一个特性拉一整套依赖合理。
//! 解析与参数构造是纯函数，所以「采样点怎么算」「ffprobe 的 JSON 怎么读」
//! 都能在没有 ffmpeg 的机器上测。

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

/// 单次最多抽取的帧数。再多对像素画没意义，只是让用户等。
pub const MAX_EXTRACT_FRAMES: usize = 256;

/// 抽帧不出来的兜底帧率：只有时长未知时才用得上。
pub const FALLBACK_FPS: f64 = 2.0;

/// 本进程内的抽帧计数。和时间戳一起保证同一台机器上两次抽帧不落在
/// 同一个目录里：光靠时间戳的话，同一纳秒发起的两回还是会撞。
static RUN_SEQ: AtomicU64 = AtomicU64::new(0);

/// 每次抽帧一个自己的子目录。ffmpeg 按固定名 `frame_0001.png` 写盘，收回时
/// 把目录里的 png 一锅端——共用一个目录的话，两次抽帧会互相覆盖，也会把
/// 上一次留下的残留帧收进来，用户拿到的就不是自己那段视频的帧了。
/// 目录来源（用户自己整理的静帧文件夹）不能这么包，它本来就不是抽出来的。
pub fn staging_dir_for(out_dir: &Path) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let seq = RUN_SEQ.fetch_add(1, Ordering::SeqCst);
    out_dir.join(format!("run-{nanos}-{seq}"))
}

/// 抽完就把这个子目录删掉：临时目录里堆着几百张几十 MB 的 png 没人管，
/// 下一次抽帧看着目录存在还会以为有残留可收。删失败不当事——帧已经读进内存了。
pub fn discard_staging(staging: &Path) {
    if staging.is_dir() {
        let _ = std::fs::remove_dir_all(staging);
    }
}

/// 探到的视频信息。任何一项都可能是 None——流媒体容器的元数据经常缺。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct VideoProbe {
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub duration_s: Option<f64>,
    pub fps: Option<f64>,
    pub frame_count: Option<u64>,
    pub codec: Option<String>,
    pub has_audio: bool,
}

/// 信息是从哪儿来的。目录来源没有 fps/duration 可言，UI 该据此改文案。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProbeSource {
    Ffprobe,
    Directory,
    None,
}

/// 探一次路径：视频文件走 ffprobe，目录走图片枚举。
pub async fn probe(path: &Path) -> Result<(VideoProbe, ProbeSource), String> {
    if path.is_dir() {
        let images = enumerate_image_dir(path)?;
        return Ok((directory_probe(&images), ProbeSource::Directory));
    }
    if !path.exists() {
        return Err(format!("no such path: {}", path.display()));
    }
    if which("ffprobe").is_none() {
        return Err(
            "ffprobe was not found on this machine; install ffmpeg or point at a folder of stills"
                .into(),
        );
    }
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || ffprobe(&path))
        .await
        .map_err(|e| format!("ffprobe task failed: {e}"))?
}

/// 抽帧的结果。`staging` 是本次抽帧自己开出来的临时目录：Some 表示用完该删；
/// None 表示帧就摆在用户自己的目录里，一个字节都不许动。少了这个区分，
/// 收尾时会把用户整理好的静帧文件夹整个删掉。
#[derive(Debug, Clone)]
pub struct ExtractedFrames {
    pub frames: Vec<PathBuf>,
    pub staging: Option<PathBuf>,
}

/// 抽帧。count <= 0 表示「全都要」，此时仍受 `MAX_EXTRACT_FRAMES` 限制。
pub async fn extract_frames(
    path: &Path,
    out_dir: &Path,
    count: usize,
) -> Result<ExtractedFrames, String> {
    if path.is_dir() {
        return Ok(ExtractedFrames {
            frames: enumerate_image_dir(path)?,
            staging: None,
        });
    }
    if !path.exists() {
        return Err(format!("no such path: {}", path.display()));
    }
    let wanted = match count {
        0 => MAX_EXTRACT_FRAMES,
        n => n.min(MAX_EXTRACT_FRAMES),
    };
    if wanted == 0 {
        return Err("extract_frames needs at least 1 frame".into());
    }
    // 每次抽帧换一个属于自己的目录。ffmpeg 按固定名写盘、收回时又把目录里的
    // png 一锅端，共用一个目录会让两次抽帧互相覆盖，也会把上一次的残留帧
    // 当成这一回的成果收回来——用户拿到的就不是自己那段视频的帧了。
    let out_dir = staging_dir_for(out_dir);
    std::fs::create_dir_all(&out_dir)
        .map_err(|e| format!("cannot create {}: {e}", out_dir.display()))?;
    if which("ffmpeg").is_none() {
        return Err(
            "ffmpeg was not found on this machine; install it to pull frames out of video".into(),
        );
    }
    let path = path.to_path_buf();
    let out_dir = out_dir.to_path_buf();
    let staging = out_dir.clone();
    let probe = probe(&path).await?.0;
    tokio::task::spawn_blocking(move || ffmpeg_extract(&path, &out_dir, wanted, &probe))
        .await
        .map_err(|e| format!("ffmpeg task failed: {e}"))?
        .map(|frames| ExtractedFrames {
            frames,
            staging: Some(staging),
        })
}

/// ffprobe 的参数。`-of json` 让输出可解析，`-v error` 压掉 banner。
pub fn ffprobe_args(path: &Path) -> Vec<String> {
    vec![
        "-v".into(),
        "error".into(),
        "-print_format".into(),
        "json".into(),
        "-show_streams".into(),
        "-show_format".into(),
        path.display().to_string(),
    ]
}

/// 跑 ffprobe 并解析。单独拆出来是为了让 `parse_probe_json` 可测。
fn ffprobe(path: &Path) -> Result<(VideoProbe, ProbeSource), String> {
    let binary = which("ffprobe").ok_or_else(|| "ffprobe not found".to_string())?;
    let args = ffprobe_args(path);
    let out = Command::new(&binary)
        .args(&args)
        .output()
        .map_err(|e| format!("cannot run ffprobe at {}: {e}", binary.display()))?;
    if !out.status.success() {
        let text = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(format!("ffprobe failed: {}", first_line(&text)));
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let p = parse_probe_json(&text)?;
    if p.width.is_none() && p.height.is_none() {
        return Err("ffprobe found no video stream in this file".into());
    }
    Ok((p, ProbeSource::Ffprobe))
}

/// 解析 ffprobe 的 JSON。字段缺失一律 None，不猜。
pub fn parse_probe_json(text: &str) -> Result<VideoProbe, String> {
    let value: serde_json::Value =
        serde_json::from_str(text).map_err(|e| format!("ffprobe output was not json: {e}"))?;
    let mut out = VideoProbe::default();
    if let Some(streams) = value.get("streams").and_then(|s| s.as_array()) {
        for s in streams {
            let is_video = s
                .get("codec_type")
                .and_then(|c| c.as_str())
                .map(|c| c == "video")
                .unwrap_or(false);
            if is_video {
                out.width = s.get("width").and_then(|v| v.as_u64()).map(|v| v as u32);
                out.height = s.get("height").and_then(|v| v.as_u64()).map(|v| v as u32);
                out.codec = s
                    .get("codec_name")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                out.fps = s
                    .get("r_frame_rate")
                    .and_then(|v| v.as_str())
                    .and_then(parse_frame_rate);
                out.frame_count = s
                    .get("nb_frames")
                    .and_then(|v| v.as_str())
                    .and_then(|v| v.parse::<u64>().ok());
                out.duration_s = s
                    .get("duration")
                    .and_then(|v| v.as_str())
                    .and_then(parse_number);
                break;
            }
        }
        out.has_audio = streams
            .iter()
            .any(|s| s.get("codec_type").and_then(|c| c.as_str()) == Some("audio"));
    }
    // 流的 duration 常常是 N/A，回落到 format 那一份。
    if out.duration_s.is_none() {
        out.duration_s = value
            .get("format")
            .and_then(|f| f.get("duration"))
            .and_then(|v| v.as_str())
            .and_then(parse_number);
    }
    // 帧数缺了就按时长和帧率反推，反推不出来就留空。
    if out.frame_count.is_none() {
        out.frame_count = out.fps.zip(out.duration_s).map(|(f, d)| (f * d) as u64);
    }
    Ok(out)
}

/// `opencl` 会给出 "30000/1001" 这种分数帧率。
pub fn parse_frame_rate(text: &str) -> Option<f64> {
    let text = text.trim();
    if text.is_empty() || text == "0/0" || text == "N/A" {
        return None;
    }
    if let Some((num, den)) = text.split_once('/') {
        let n: f64 = num.trim().parse().ok()?;
        let d: f64 = den.trim().parse().ok()?;
        if d == 0.0 {
            return None;
        }
        Some(n / d)
    } else {
        parse_number(text)
    }
}

fn parse_number(text: &str) -> Option<f64> {
    let text = text.trim();
    if text.is_empty() || text == "N/A" {
        return None;
    }
    text.parse::<f64>()
        .ok()
        .filter(|v| v.is_finite() && *v > 0.0)
}

/// 在时长上均匀取 count 个采样点。取每段中点而不是端点：
/// 片头常有黑场，末帧常是重复帧，取中点两头都避开。
pub fn sample_times(duration_s: f64, count: usize) -> Vec<f64> {
    if !(duration_s.is_finite() && duration_s > 0.0) || count == 0 {
        return Vec::new();
    }
    let n = count.min(MAX_EXTRACT_FRAMES) as f64;
    let step = duration_s / n;
    (0..n as usize).map(|i| (i as f64 + 0.5) * step).collect()
}

/// 由采样点推出 ffmpeg 的 `fps` 滤镜参数：count / duration 近似均匀。
pub fn frames_fps(count: usize, duration_s: Option<f64>) -> Option<f64> {
    let d = duration_s?;
    if !(d.is_finite() && d > 0.0) || count == 0 {
        return None;
    }
    Some((count as f64) / d)
}

fn ffmpeg_extract(
    path: &Path,
    out_dir: &Path,
    wanted: usize,
    probe: &VideoProbe,
) -> Result<Vec<PathBuf>, String> {
    let binary = which("ffmpeg").ok_or_else(|| "ffmpeg not found".to_string())?;
    let pattern = out_dir.join("frame_%04d.png");
    let mut args = vec![
        "-y".into(),
        "-v".into(),
        "error".into(),
        "-i".into(),
        path.display().to_string(),
    ];
    // 有时长就按均匀采样定帧率；没有时长只能退回固定帧率多抽一些再截断。
    match frames_fps(wanted, probe.duration_s) {
        Some(fps) => {
            args.push("-vf".into());
            args.push(format!("fps={fps:.6}"));
        }
        None => {
            args.push("-vf".into());
            args.push(format!("fps={FALLBACK_FPS:.6}"));
        }
    }
    args.push("-frames:v".into());
    args.push(wanted.to_string());
    args.push(pattern.display().to_string());
    let out = Command::new(&binary)
        .args(&args)
        .output()
        .map_err(|e| format!("cannot run ffmpeg at {}: {e}", binary.display()))?;
    if !out.status.success() {
        let text = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(format!("ffmpeg failed: {}", first_line(&text)));
    }
    let written = list_pngs(out_dir);
    if written.is_empty() {
        return Err("ffmpeg reported success but wrote no frames".into());
    }
    Ok(written.into_iter().take(wanted).collect())
}

/// 把目录当帧序列读。这是「没有 ffmpeg」时的退路，也方便用户整理好静帧再喂进来。
pub fn enumerate_image_dir(dir: &Path) -> Result<Vec<PathBuf>, String> {
    let entries =
        std::fs::read_dir(dir).map_err(|e| format!("cannot read {}: {e}", dir.display()))?;
    let mut out: Vec<PathBuf> = Vec::new();
    for entry in entries {
        let Ok(entry) = entry else { continue };
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())
            .unwrap_or_default();
        if matches!(
            ext.as_str(),
            "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp"
        ) {
            out.push(path);
        }
    }
    // 按文件名排序：帧序列几乎总是 frame_0001.png 这类可排序命名，且用户重命名后
    // 的字典序就是他们心里的时间序。
    out.sort();
    if out.is_empty() {
        return Err(format!("no image files in {}", dir.display()));
    }
    Ok(out.into_iter().take(MAX_EXTRACT_FRAMES).collect())
}

fn directory_probe(images: &[PathBuf]) -> VideoProbe {
    // 目录来源不读位图头：那会把解码成本塞进一次「用户只是想看看有几帧」的调用里。
    VideoProbe {
        frame_count: Some(images.len() as u64),
        ..Default::default()
    }
}

fn list_pngs(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<PathBuf> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.extension()
                .and_then(|e| e.to_str())
                .map(|e| e.eq_ignore_ascii_case("png"))
                .unwrap_or(false)
        })
        .collect();
    out.sort();
    out
}

fn first_line(text: &str) -> String {
    text.lines().next().unwrap_or("").trim().to_string()
}

/// 在 PATH 上找一个可执行文件。找不到就返回 None，调用方据此给出可操作的提示。
pub fn which(binary: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        if dir.as_os_str().is_empty() {
            continue;
        }
        let candidate = dir.join(binary);
        if candidate.is_file() && is_executable(&candidate) {
            return Some(candidate);
        }
    }
    None
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .map(|m| m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable(_path: &Path) -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn probe_json() -> &'static str {
        r#"{
            "streams": [
                {"index": 0, "codec_type": "video", "codec_name": "h264", "width": 640, "height": 480, "r_frame_rate": "30000/1001", "nb_frames": "600", "duration": "20.020000"},
                {"index": 1, "codec_type": "audio", "codec_name": "aac"}
            ],
            "format": {"duration": "20.020000", "format_name": "mov,mp4,m4a,3gp,3g2,mj2"}
        }"#
    }

    #[test]
    fn parses_the_video_stream_and_notices_the_audio_track() {
        let p = parse_probe_json(probe_json()).expect("parses");
        assert_eq!(p.width, Some(640));
        assert_eq!(p.height, Some(480));
        assert_eq!(p.codec.as_deref(), Some("h264"));
        assert!(p.has_audio);
        assert_eq!(p.frame_count, Some(600));
        assert!((p.fps.unwrap() - 29.97).abs() < 0.01, "{:?}", p.fps);
        assert!((p.duration_s.unwrap() - 20.02).abs() < 1e-9);
    }

    #[test]
    fn falls_back_to_the_container_duration_when_the_stream_hides_it() {
        let text = r#"{"streams":[{"codec_type":"video","width":16,"height":16,"duration":"N/A","nb_frames":"N/A","r_frame_rate":"0/0"}]}"#;
        let p = parse_probe_json(text).expect("parses");
        assert_eq!(p.duration_s, None, "container had no duration either");
        assert_eq!(p.frame_count, None);
        assert_eq!(p.fps, None);
        assert_eq!(p.width, Some(16));
    }

    #[test]
    fn audio_only_output_is_not_mistaken_for_video() {
        let text = r#"{"streams":[{"codec_type":"audio","codec_name":"aac"}]}"#;
        let p = parse_probe_json(text).expect("parses");
        // 音频轨要报出来，但一个视频字段都不该有。
        assert!(p.has_audio);
        assert_eq!(p.width, None);
        assert_eq!(p.height, None);
        assert_eq!(p.fps, None);
        assert_eq!(p.frame_count, None);
        assert_eq!(p.codec, None);
    }

    #[test]
    fn frame_rate_handles_fractions_and_junk() {
        assert_eq!(parse_frame_rate("30000/1001"), Some(29.97002997002997));
        assert_eq!(parse_frame_rate("24"), Some(24.0));
        assert_eq!(parse_frame_rate("N/A"), None);
        assert_eq!(parse_frame_rate("0/0"), None);
        assert_eq!(parse_frame_rate("30/0"), None);
        assert_eq!(parse_frame_rate(""), None);
    }

    #[test]
    fn samples_land_on_segment_midpoints_away_from_black_head_and_repeat_tail() {
        let times = sample_times(10.0, 4);
        assert_eq!(times.len(), 4);
        assert!((times[0] - 1.25).abs() < 1e-9);
        assert!(*times.last().unwrap() < 10.0);
        for pair in times.windows(2) {
            assert!((pair[1] - pair[0] - 2.5).abs() < 1e-9);
        }
    }

    #[test]
    fn sampling_degrades_quietly_on_nonsense_durations() {
        assert!(sample_times(0.0, 4).is_empty());
        assert!(sample_times(-1.0, 4).is_empty());
        assert!(sample_times(f64::NAN, 4).is_empty());
        assert!(sample_times(f64::INFINITY, 4).is_empty());
        assert!(sample_times(10.0, 0).is_empty());
    }

    #[test]
    fn fps_is_derived_from_the_requested_count_and_clamped_to_the_cap() {
        let fps = frames_fps(8, Some(4.0)).expect("count and duration");
        assert!((fps - 2.0).abs() < 1e-9, "{fps}");
        assert_eq!(frames_fps(0, Some(4.0)), None);
        assert_eq!(frames_fps(8, None), None);
        assert_eq!(frames_fps(8, Some(0.0)), None);
        let big = frames_fps(MAX_EXTRACT_FRAMES * 10, Some(4.0)).expect("clamped count");
        assert!(big > 0.0);
    }

    #[test]
    fn ffprobe_args_json_and_stay_quiet() {
        let args = ffprobe_args(Path::new("/tmp/clip.mp4"));
        assert!(args.contains(&"json".to_string()));
        assert!(args.contains(&"-show_streams".to_string()));
        assert!(args.iter().any(|a| a == "/tmp/clip.mp4"));
        // banner 必须压掉，否则 stderr 里全是版本号，出错时看不出所以然。
        assert!(args.contains(&"error".to_string()));
    }

    #[test]
    fn a_missing_path_is_reported_named_rather_than_as_an_opaque_failure() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let err = rt
            .block_on(probe(Path::new("/definitely/not/here.mp4")))
            .unwrap_err();
        assert!(err.contains("/definitely/not/here.mp4"), "{err}");
    }

    #[test]
    fn which_finds_a_common_binary_or_reports_none_cleanly() {
        // 不假设测试机装了 ffmpeg：只要求它别 panic，且找到的东西必须真能跑。
        if let Some(found) = which("ls") {
            assert!(found.is_file(), "{found:?}");
        }
        assert!(which("aipixel-no-such-binary-xyz").is_none());
    }

    #[test]
    fn a_directory_of_stills_becomes_a_sequence_without_any_process() {
        // 只在环境变量给了目录时才跑，避免 CI 依赖 tmp 目录。
        let Ok(dir) = std::env::var("AIPIXEL_STILLS") else {
            return;
        };
        let images = enumerate_image_dir(Path::new(&dir)).expect("lists stills");
        assert!(!images.is_empty());
    }
}
