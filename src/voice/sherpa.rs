//! Offline speech with sherpa-onnx.
//!
//! One optional install gives OSA on-device speech recognition and a voice,
//! with no account and no network after setup. It runs sherpa-onnx's own
//! command-line tools as child processes, so OSA needs no native bindings.
//! Everything lives under `~/.osagent/voice/sherpa`; deleting that folder
//! removes the feature.

use serde::Serialize;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Mutex;
use std::time::Duration;

/// Pinned release assets, each checked against its release page.
const RUNTIME_URL: &str = "https://github.com/k2-fsa/sherpa-onnx/releases/download/v1.13.8/sherpa-onnx-v1.13.8-win-x64-shared-MD-Release.tar.bz2";
const STT_URL: &str = "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-nemo-parakeet-tdt-0.6b-v2-int8.tar.bz2";
const TTS_URL: &str = "https://github.com/k2-fsa/sherpa-onnx/releases/download/tts-models/vits-piper-en_US-amy-low-int8.tar.bz2";

// Parakeet TDT 0.6B v2 is run as a non-streaming transducer. It is much more
// accurate on conversational English than the small streaming model it replaced.
const RECOGNIZER_EXE: &str = "sherpa-onnx-offline.exe";
const TTS_EXE: &str = "sherpa-onnx-offline-tts.exe";
const ENCODER: &str = "encoder.int8.onnx";
const DECODER: &str = "decoder.int8.onnx";
const JOINER: &str = "joiner.int8.onnx";
const TTS_MODEL: &str = "en_US-amy-low.onnx";

/// Download sizes in MB. They weight the progress bar across the three downloads.
const RUNTIME_MB: f64 = 20.0;
const STT_MB: f64 = 482.0;
const TTS_MB: f64 = 21.0;
/// Total download shown to the user before they start the install.
pub const DOWNLOAD_MB: u64 = 525;

/// Longest text sent to the voice in one call.
const MAX_SPEECH_CHARS: usize = 1000;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[derive(Debug, Clone, Default, Serialize)]
pub struct SherpaProgress {
    pub running: bool,
    /// Human-readable step, for example "Downloading the voice".
    pub phase: String,
    /// Overall install progress, 0 to 100.
    pub percent: f64,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SherpaStatus {
    pub installed: bool,
    pub download_mb: u64,
    pub progress: SherpaProgress,
}

static PROGRESS: Mutex<SherpaProgress> = Mutex::new(SherpaProgress {
    running: false,
    phase: String::new(),
    percent: 0.0,
    error: None,
});

pub fn root() -> PathBuf {
    super::get_models_dir().join("sherpa")
}

fn runtime_bin(root: &Path) -> PathBuf {
    root.join("runtime").join("bin")
}

fn stt_dir(root: &Path) -> PathBuf {
    root.join("models").join("stt")
}

fn tts_dir(root: &Path) -> PathBuf {
    root.join("models").join("tts")
}

/// True only when every file the two tools need is present.
pub fn is_installed_in(root: &Path) -> bool {
    let bin = runtime_bin(root);
    let stt = stt_dir(root);
    let tts = tts_dir(root);
    bin.join(RECOGNIZER_EXE).is_file()
        && bin.join(TTS_EXE).is_file()
        && stt.join("tokens.txt").is_file()
        && stt.join(ENCODER).is_file()
        && stt.join(DECODER).is_file()
        && stt.join(JOINER).is_file()
        && tts.join(TTS_MODEL).is_file()
        && tts.join("tokens.txt").is_file()
        && tts.join("espeak-ng-data").is_dir()
}

pub fn is_installed() -> bool {
    is_installed_in(&root())
}

pub fn progress() -> SherpaProgress {
    PROGRESS.lock().map(|p| p.clone()).unwrap_or_default()
}

fn update_progress(update: impl FnOnce(&mut SherpaProgress)) {
    if let Ok(mut progress) = PROGRESS.lock() {
        update(&mut progress);
    }
}

pub fn status() -> SherpaStatus {
    SherpaStatus {
        installed: is_installed(),
        download_mb: DOWNLOAD_MB,
        progress: progress(),
    }
}

/// Claims the install slot. Returns false when an install is already running.
fn begin_install() -> bool {
    let mut claimed = false;
    update_progress(|p| {
        if !p.running {
            p.running = true;
            p.percent = 0.0;
            p.error = None;
            p.phase = "Starting".to_string();
            claimed = true;
        }
    });
    claimed
}

/// Downloads the runtime and both models, then moves them into place. Files go
/// to a staging folder first, so an interrupted download never leaves a
/// half-working install.
pub async fn install() -> Result<(), String> {
    if !begin_install() {
        return Err("A speech install is already running.".to_string());
    }
    let result = install_inner().await;
    update_progress(|p| {
        p.running = false;
        match &result {
            Ok(()) => {
                p.percent = 100.0;
                p.phase = "Ready".to_string();
            }
            Err(error) => p.error = Some(error.clone()),
        }
    });
    result
}

async fn install_inner() -> Result<(), String> {
    let root = root();
    let stage = root.join(".staging");
    let _ = tokio::fs::remove_dir_all(&stage).await;
    tokio::fs::create_dir_all(&stage)
        .await
        .map_err(|e| format!("Could not create {}: {e}", stage.display()))?;

    let total_mb = RUNTIME_MB + STT_MB + TTS_MB;
    let steps = [
        (
            "runtime",
            "Downloading the speech runtime",
            RUNTIME_URL,
            RUNTIME_MB,
        ),
        ("stt", "Downloading the listening model", STT_URL, STT_MB),
        ("tts", "Downloading the voice", TTS_URL, TTS_MB),
    ];
    let mut offset = 0.0;
    for (name, label, url, weight_mb) in steps {
        let share = weight_mb / total_mb * 100.0;
        let start = offset;
        update_progress(|p| {
            p.phase = label.to_string();
            p.percent = start;
        });
        let archive = stage.join(format!("{name}.tar.bz2"));
        download(url, &archive, |fraction| {
            update_progress(|p| p.percent = start + share * fraction);
        })
        .await?;

        let unpacked = stage.join(format!("{name}-unpacked"));
        let target = stage.join(name);
        let archive_path = archive.clone();
        let unpacked_path = unpacked.clone();
        let top =
            tokio::task::spawn_blocking(move || unpack_bz2_tar(&archive_path, &unpacked_path))
                .await
                .map_err(|e| e.to_string())??;
        std::fs::rename(&top, &target)
            .map_err(|e| format!("Could not place the {name} files: {e}"))?;
        let _ = std::fs::remove_file(&archive);
        let _ = std::fs::remove_dir_all(&unpacked);
        offset += share;
    }

    update_progress(|p| {
        p.phase = "Finishing".to_string();
        p.percent = 99.0;
    });
    // Replace the old install only once the new files are all staged.
    let _ = std::fs::remove_dir_all(root.join("runtime"));
    let _ = std::fs::remove_dir_all(root.join("models"));
    std::fs::create_dir_all(root.join("models"))
        .map_err(|e| format!("Could not create the models folder: {e}"))?;
    std::fs::rename(stage.join("runtime"), root.join("runtime"))
        .map_err(|e| format!("Could not place the speech runtime: {e}"))?;
    std::fs::rename(stage.join("stt"), stt_dir(&root))
        .map_err(|e| format!("Could not place the listening model: {e}"))?;
    std::fs::rename(stage.join("tts"), tts_dir(&root))
        .map_err(|e| format!("Could not place the voice: {e}"))?;
    let _ = std::fs::remove_dir_all(&stage);

    if !is_installed_in(&root) {
        return Err("The download finished, but some speech files are missing.".to_string());
    }
    Ok(())
}

async fn download(url: &str, dest: &Path, mut on_progress: impl FnMut(f64)) -> Result<(), String> {
    use futures::StreamExt;
    use tokio::io::AsyncWriteExt;

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(60 * 60))
        .user_agent("OSAgent")
        .build()
        .map_err(|e| e.to_string())?;
    let response = client
        .get(url)
        .send()
        .await
        .and_then(|r| r.error_for_status())
        .map_err(|e| format!("Download failed: {e}"))?;
    let total = response.content_length().unwrap_or(0);
    let mut file = tokio::fs::File::create(dest)
        .await
        .map_err(|e| format!("Could not write {}: {e}", dest.display()))?;
    let mut stream = response.bytes_stream();
    let mut done: u64 = 0;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("Download interrupted: {e}"))?;
        file.write_all(&chunk)
            .await
            .map_err(|e| format!("Could not write the download: {e}"))?;
        done += chunk.len() as u64;
        if total > 0 {
            on_progress((done as f64 / total as f64).min(1.0));
        }
    }
    file.flush().await.map_err(|e| e.to_string())?;
    Ok(())
}

/// Unpacks a `.tar.bz2` and returns the single top-level folder it contains.
fn unpack_bz2_tar(archive: &Path, into: &Path) -> Result<PathBuf, String> {
    let _ = std::fs::remove_dir_all(into);
    std::fs::create_dir_all(into).map_err(|e| e.to_string())?;
    let file = std::fs::File::open(archive).map_err(|e| e.to_string())?;
    tar::Archive::new(bzip2::read::BzDecoder::new(file))
        .unpack(into)
        .map_err(|e| format!("Could not unpack the download: {e}"))?;
    std::fs::read_dir(into)
        .map_err(|e| e.to_string())?
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .find(|path| path.is_dir())
        .ok_or_else(|| "The download did not contain any files.".to_string())
}

/// Removes every installed speech file. Other voice features are untouched.
pub fn remove() -> Result<(), String> {
    match std::fs::remove_dir_all(root()) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("Could not remove the speech files: {e}")),
    }
    update_progress(|p| *p = SherpaProgress::default());
    Ok(())
}

fn path_arg(path: impl AsRef<Path>) -> String {
    path.as_ref().display().to_string()
}

async fn run_tool(exe: PathBuf, args: Vec<String>) -> Result<String, String> {
    let output = tokio::task::spawn_blocking(move || {
        let mut command = std::process::Command::new(&exe);
        command.args(&args).stdin(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(CREATE_NO_WINDOW);
        }
        command.output()
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| format!("Could not start the speech tool: {e}"))?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    if !output.status.success() {
        let tail: String = stderr
            .chars()
            .rev()
            .take(300)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        return Err(format!("The speech tool failed: {}", tail.trim()));
    }
    // The offline tool prints its result on stdout, and the streaming one on
    // stderr, so both streams are kept and the result is found in either.
    Ok(format!(
        "{stdout}
{stderr}"
    ))
}

/// Transcribes a 16 kHz mono WAV file.
pub async fn transcribe(wav: &Path) -> Result<String, String> {
    transcribe_with(&root(), wav).await
}

async fn transcribe_with(root: &Path, wav: &Path) -> Result<String, String> {
    if !is_installed_in(root) {
        return Err(
            "Offline voice is not set up yet. Open Settings > Voice and set it up.".to_string(),
        );
    }
    let stt = stt_dir(root);
    let threads = std::thread::available_parallelism()
        .map(|n| n.get().min(4))
        .unwrap_or(2);
    let args = vec![
        "--model-type=nemo_transducer".to_string(),
        format!("--tokens={}", path_arg(stt.join("tokens.txt"))),
        format!("--encoder={}", path_arg(stt.join(ENCODER))),
        format!("--decoder={}", path_arg(stt.join(DECODER))),
        format!("--joiner={}", path_arg(stt.join(JOINER))),
        format!("--num-threads={threads}"),
        path_arg(wav),
    ];
    let stdout = run_tool(runtime_bin(root).join(RECOGNIZER_EXE), args).await?;
    parse_recognizer_output(&stdout)
}

/// The final result is one JSON object on its own line, after the config dump.
pub fn parse_recognizer_output(stdout: &str) -> Result<String, String> {
    let line = stdout
        .lines()
        .rev()
        .map(str::trim)
        .find(|line| line.starts_with('{') && line.contains("\"text\""))
        .ok_or_else(|| "The speech tool returned no result.".to_string())?;
    let value: serde_json::Value =
        serde_json::from_str(line).map_err(|e| format!("Could not read the speech result: {e}"))?;
    Ok(value["text"].as_str().unwrap_or("").trim().to_string())
}

/// Writes `text` as speech into `out` (a WAV file).
pub async fn synthesize(text: &str, out: &Path) -> Result<(), String> {
    synthesize_with(&root(), text, out).await
}

async fn synthesize_with(root: &Path, text: &str, out: &Path) -> Result<(), String> {
    if !is_installed_in(root) {
        return Err(
            "Offline voice is not set up yet. Open Settings > Voice and set it up.".to_string(),
        );
    }
    let spoken = sanitize_for_speech(text);
    if spoken.is_empty() {
        return Err("There is nothing to say.".to_string());
    }
    let tts = tts_dir(root);
    let args = vec![
        format!("--vits-model={}", path_arg(tts.join(TTS_MODEL))),
        format!("--vits-tokens={}", path_arg(tts.join("tokens.txt"))),
        format!("--vits-data-dir={}", path_arg(tts.join("espeak-ng-data"))),
        format!("--output-filename={}", path_arg(out)),
        spoken,
    ];
    run_tool(runtime_bin(root).join(TTS_EXE), args).await?;
    if !out.is_file() {
        return Err("The voice did not produce audio.".to_string());
    }
    Ok(())
}

/// One line of plain text, no leading dashes (they would read as options), and
/// a length cap.
pub fn sanitize_for_speech(text: &str) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    flat.trim_start_matches('-')
        .trim()
        .chars()
        .take(MAX_SPEECH_CHARS)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_final_json_result_after_the_config_dump() {
        let stdout = "OnlineRecognizerConfig(model_config=...)\nRecognizer created in 0.7 s\n\
            Number of threads: 1, Elapsed seconds: 0.39\n\
            { \"text\": \"HELLO THERE\", \"tokens\": [], \"is_final\": false }\n";
        assert_eq!(parse_recognizer_output(stdout).unwrap(), "HELLO THERE");
    }

    #[test]
    fn empty_speech_is_a_result_not_an_error() {
        let stdout = "{ \"text\": \"\", \"tokens\": [] }\n";
        assert_eq!(parse_recognizer_output(stdout).unwrap(), "");
        assert!(parse_recognizer_output("no json here").is_err());
    }

    #[test]
    fn speech_text_is_one_line_without_leading_dashes() {
        assert_eq!(sanitize_for_speech("  --rm -rf\n  now "), "rm -rf now");
        assert_eq!(sanitize_for_speech(" \n\t "), "");
        assert_eq!(
            sanitize_for_speech(&"a".repeat(5000)).chars().count(),
            MAX_SPEECH_CHARS
        );
    }

    #[test]
    fn an_empty_folder_is_not_an_install() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!is_installed_in(dir.path()));
    }

    /// Runs the real tools against an install folder. Point OSA_SHERPA_TEST_ROOT
    /// at a folder with the same layout as `root()`, then run with `--ignored`.
    #[tokio::test]
    #[ignore]
    async fn real_tools_round_trip() {
        let Some(root) = std::env::var_os("OSA_SHERPA_TEST_ROOT") else {
            return;
        };
        let root = PathBuf::from(root);
        assert!(
            is_installed_in(&root),
            "install layout incomplete under {}",
            root.display()
        );

        let dir = tempfile::tempdir().unwrap();
        let wav = dir.path().join("speech.wav");
        synthesize_with(&root, "Hello from OSA, the offline voice works.", &wav)
            .await
            .unwrap();
        // The recogniser must run and answer. Quality varies with the audio, so
        // the accuracy check uses the model's own sample when one is provided.
        let text = transcribe_with(&root, &wav).await.unwrap();
        let _ = std::io::stdout().flush();
        println!("recognised synthetic speech as: {text}");
        if let Some(sample) = std::env::var_os("OSA_SHERPA_TEST_WAV") {
            let sample = transcribe_with(&root, Path::new(&sample)).await.unwrap();
            let lower = sample.to_lowercase();
            assert!(
                lower.contains("kitchen") && lower.contains("timer"),
                "recognised sample as: {sample}"
            );
        }
    }
}
