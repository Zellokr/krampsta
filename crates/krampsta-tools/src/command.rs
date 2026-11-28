//! `run_command`: a shell command in the project folder, its output streamed to the user as it
//! comes and handed to the model (head and tail) when it ends.
//!
//! The command runs as a process tree that is killed whole (a job object on Windows, a process
//! group on Unix) when it times out, when the run is cancelled, or once its shell exits, so no
//! child is left behind holding the pipes. Two threads drain stdout and stderr through a bounded
//! channel; the output reaches the UI in batches (every 50 ms or 16 KB), never line by line, and
//! the model's copy keeps a fixed head and a ring-buffered tail, so memory stays flat however
//! much a command prints.

use std::collections::VecDeque;
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};

use krampsta_agent::{Approval, Call};
use process_wrap::std::{ChildWrapper, CommandWrap};
use serde::Deserialize;

use crate::project::Project;
use crate::sandbox::{Confinement, Sandbox};

pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);
pub const MAX_TIMEOUT: Duration = Duration::from_secs(600);
/// How much of the output the model gets: the start, and the end (where errors usually are).
const HEAD_BYTES: usize = 8 * 1024;
const TAIL_BYTES: usize = 24 * 1024;
/// How often (at most) output goes to the UI, and the batch size that sends it sooner.
const FLUSH_EVERY: Duration = Duration::from_millis(50);
const FLUSH_BYTES: usize = 16 * 1024;

#[derive(Deserialize)]
pub struct Args {
    pub command: String,
    #[serde(default)]
    timeout_ms: Option<u64>,
    #[serde(default)]
    pub pending: bool,
}

/// How a command ended.
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    Exited(Option<i32>),
    TimedOut(Duration),
    Stopped,
}

impl Outcome {
    pub fn describe(&self) -> String {
        match self {
            Outcome::Exited(Some(code)) => format!("exit code {code}"),
            Outcome::Exited(None) => "ended by a signal".into(),
            Outcome::TimedOut(after) => format!("timed out after {} s and was stopped", after.as_secs()),
            Outcome::Stopped => "stopped by the user".into(),
        }
    }
}

/// The shell commands run in, as the tool's description tells the model.
pub const SHELL: &str = if cfg!(windows) { "PowerShell" } else { "sh" };

/// Runs `args.command` to its end, inside `sandbox` if any; returns the output for the model,
/// how it ended and how confined it was. With `cargo_target`, Cargo builds there (checks of
/// pending edits, run in a copy of the project).
pub fn run(
    project: &Project,
    args: &Args,
    cargo_target: Option<&Path>,
    sandbox: Option<&Sandbox>,
    call: &Call<'_>,
) -> Result<(String, Outcome, Confinement), String> {
    let timeout = args
        .timeout_ms
        .map_or(DEFAULT_TIMEOUT, |ms| Duration::from_millis(ms).clamp(Duration::from_secs(1), MAX_TIMEOUT));
    if call.cancel.is_cancelled() {
        return Ok((String::new(), Outcome::Stopped, Confinement::Off));
    }
    let start = || spawn(project, &args.command, cargo_target);
    let (started, confinement) = match sandbox {
        Some(sandbox) => sandbox.spawn(start)?,
        None => (start(), Confinement::Off),
    };
    let mut child = started.map_err(|e| format!("the command could not start: {e}"))?;
    let (tx, rx) = mpsc::sync_channel::<Vec<u8>>(64);
    let readers = [child.stdout().take().map(|p| reader(p, tx.clone())), child.stderr().take().map(|p| reader(p, tx))];

    let started = Instant::now();
    let mut capture = Capture::default();
    let mut batch = Batch::default();
    let mut outcome = None;
    let mut ended = None::<Instant>;
    loop {
        // Killed but something still holds a pipe (it left the job): stop waiting for it.
        if ended.is_some_and(|t| t.elapsed() > Duration::from_secs(5)) {
            break;
        }
        match rx.recv_timeout(FLUSH_EVERY) {
            Ok(chunk) => {
                capture.push(&chunk);
                batch.push(&chunk, call.output);
            }
            Err(RecvTimeoutError::Timeout) => {}
            // Both pipes closed: the whole tree is gone (or let go of them).
            Err(RecvTimeoutError::Disconnected) => break,
        }
        batch.flush_if_due(call.output);
        if outcome.is_none() {
            if call.cancel.is_cancelled() {
                outcome = Some(Outcome::Stopped);
            } else if started.elapsed() >= timeout {
                outcome = Some(Outcome::TimedOut(timeout));
            } else if let Ok(Some(status)) = child.try_wait() {
                outcome = Some(Outcome::Exited(status.code()));
            }
            // However it ended, nothing it started may outlive it and keep the pipes open.
            if outcome.is_some() {
                let _ = child.kill();
                ended = Some(Instant::now());
            }
        }
    }
    batch.flush(call.output);
    // A reader still blocked on a pipe someone else holds is left to finish on its own.
    for reader in readers.into_iter().flatten().filter(|r| r.is_finished()) {
        let _ = reader.join();
    }
    let outcome = match outcome {
        Some(outcome) => outcome,
        None => Outcome::Exited(child.wait().map_err(|e| format!("could not wait for the command: {e}"))?.code()),
    };
    Ok((capture.into_text(), outcome, confinement))
}

/// The AI providers' API keys, as Krampsta (and the user's other tools) read them from the
/// environment. A build or a test never needs them, and a command's output goes to the model:
/// `echo $env:OPENAI_API_KEY` would send the key along. Commands don't get them.
pub const MODEL_KEYS: [&str; 12] = [
    "OPENAI_API_KEY",
    "ANTHROPIC_API_KEY",
    "GEMINI_API_KEY",
    "GOOGLE_API_KEY",
    "AZURE_OPENAI_API_KEY",
    "OPENROUTER_API_KEY",
    "GROQ_API_KEY",
    "MISTRAL_API_KEY",
    "DEEPSEEK_API_KEY",
    "XAI_API_KEY",
    "TOGETHER_API_KEY",
    "COHERE_API_KEY",
];

fn hide_model_keys(cmd: &mut Command) {
    for name in MODEL_KEYS {
        cmd.env_remove(name);
    }
}

fn spawn(project: &Project, command: &str, cargo_target: Option<&Path>) -> std::io::Result<Box<dyn ChildWrapper>> {
    let mut cmd = shell(command);
    hide_model_keys(&mut cmd);
    if let Some(target) = cargo_target {
        cmd.env("CARGO_TARGET_DIR", target);
    }
    cmd.current_dir(project.root()).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut wrapped = CommandWrap::from(cmd);
    #[cfg(windows)]
    {
        use process_wrap::std::{CreationFlags, JobObject};
        use windows::Win32::System::Threading::CREATE_NO_WINDOW;
        // No console window flashes up behind the app.
        wrapped.wrap(CreationFlags(CREATE_NO_WINDOW)).wrap(JobObject);
    }
    #[cfg(unix)]
    wrapped.wrap(process_wrap::std::ProcessGroup::leader());
    wrapped.spawn()
}

#[cfg(windows)]
fn shell(command: &str) -> Command {
    let mut cmd = Command::new("powershell.exe");
    // UTF-8 everywhere, so accents survive: the output down the pipe (not the console's code
    // page), what is piped to programs, and the files cmdlets read and write — Windows
    // PowerShell 5.1 reads a UTF-8 file without a BOM as ANSI ("guía" came back as "guÃ­a").
    let script = format!(
        "[Console]::OutputEncoding = [Text.Encoding]::UTF8; $OutputEncoding = [Text.Encoding]::UTF8; \
         $PSDefaultParameterValues['*:Encoding'] = 'utf8'; {command}"
    );
    cmd.args(["-NoLogo", "-NoProfile", "-NonInteractive", "-Command", &script]);
    cmd
}

#[cfg(not(windows))]
fn shell(command: &str) -> Command {
    let mut cmd = Command::new("sh");
    cmd.args(["-c", command]);
    cmd
}

/// A thread that reads `pipe` in 8 KB chunks into `tx` until it closes.
fn reader(mut pipe: impl Read + Send + 'static, tx: mpsc::SyncSender<Vec<u8>>) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let mut buf = vec![0; 8 * 1024];
        while let Ok(n) = pipe.read(&mut buf) {
            if n == 0 || tx.send(buf[..n].to_vec()).is_err() {
                break;
            }
        }
    })
}

/// Output on its way to the UI: bytes since the last flush, and an incomplete UTF-8 character
/// carried to the next batch so it is never cut in two.
#[derive(Default)]
struct Batch {
    bytes: Vec<u8>,
    last: Option<Instant>,
}

impl Batch {
    fn push(&mut self, chunk: &[u8], out: &(dyn Fn(&str) + Send + Sync)) {
        self.bytes.extend_from_slice(chunk);
        if self.bytes.len() >= FLUSH_BYTES {
            self.flush(out);
        }
    }

    fn flush_if_due(&mut self, out: &(dyn Fn(&str) + Send + Sync)) {
        if self.last.is_none_or(|t| t.elapsed() >= FLUSH_EVERY) {
            self.flush(out);
        }
    }

    fn flush(&mut self, out: &(dyn Fn(&str) + Send + Sync)) {
        self.last = Some(Instant::now());
        if self.bytes.is_empty() {
            return;
        }
        let complete = match std::str::from_utf8(&self.bytes) {
            Ok(_) => self.bytes.len(),
            // Up to 3 bytes of a character still arriving; anything else invalid is shown lossily.
            Err(e) if e.error_len().is_none() => e.valid_up_to(),
            Err(_) => self.bytes.len(),
        };
        // Windows line ends shown as plain ones; once per batch, not per line.
        out(&String::from_utf8_lossy(&self.bytes[..complete]).replace("\r\n", "\n"));
        self.bytes.drain(..complete);
    }
}

/// The model's copy of the output: the first [`HEAD_BYTES`] and a ring of the last
/// [`TAIL_BYTES`], with a count of what fell between.
#[derive(Default)]
struct Capture {
    head: Vec<u8>,
    tail: VecDeque<u8>,
    dropped: usize,
}

impl Capture {
    fn push(&mut self, mut chunk: &[u8]) {
        if self.head.len() < HEAD_BYTES {
            let take = chunk.len().min(HEAD_BYTES - self.head.len());
            self.head.extend_from_slice(&chunk[..take]);
            chunk = &chunk[take..];
        }
        self.tail.extend(chunk);
        let over = self.tail.len().saturating_sub(TAIL_BYTES);
        self.tail.drain(..over);
        self.dropped += over;
    }

    fn into_text(self) -> String {
        let mut bytes = self.head;
        if self.dropped > 0 {
            bytes.extend_from_slice(format!("\n… ({} bytes of output left out) …\n", self.dropped).as_bytes());
        }
        bytes.extend(self.tail);
        String::from_utf8_lossy(&bytes).into_owned()
    }
}

/// How a call was allowed, as the command log writes it.
pub fn approval_text(approval: &Approval) -> String {
    match approval {
        Approval::NotNeeded => "not needed".into(),
        Approval::Rule(rule) => format!("rule `{rule}`"),
        Approval::Once => "the user, once".into(),
        Approval::Always => "the user, always".into(),
        Approval::Yolo => "yolo mode".into(),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use krampsta_agent::CancellationToken;

    use super::*;

    fn run_it(command: &str, timeout_ms: Option<u64>, cancel: &CancellationToken) -> (String, Outcome, String) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("marker.txt"), "").unwrap();
        let project = Project::open(dir.path()).unwrap();
        let streamed = Mutex::new(String::new());
        let output = |text: &str| streamed.lock().unwrap().push_str(text);
        let call = Call { cancel, approval: &Approval::Once, output: &output };
        let (text, outcome, _) =
            run(&project, &Args { command: command.into(), timeout_ms, pending: false }, None, None, &call).unwrap();
        let streamed = streamed.into_inner().unwrap();
        (text, outcome, streamed)
    }

    #[test]
    fn commands_never_see_the_model_providers_keys() {
        let echo = if cfg!(windows) {
            "Write-Output \"[$env:OPENAI_API_KEY][$env:ANTHROPIC_API_KEY][$env:KRAMPSTA_TEST_KEEP]\""
        } else {
            "echo \"[$OPENAI_API_KEY][$ANTHROPIC_API_KEY][$KRAMPSTA_TEST_KEEP]\""
        };
        let mut cmd = shell(echo);
        cmd.env("OPENAI_API_KEY", "sk-secret").env("ANTHROPIC_API_KEY", "sk-ant").env("KRAMPSTA_TEST_KEEP", "kept");
        hide_model_keys(&mut cmd);
        let out = cmd.output().unwrap();
        let said = String::from_utf8_lossy(&out.stdout);
        assert_eq!(said.trim(), "[][][kept]", "the keys are gone, the rest of the environment stays");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_sandboxed_command_writes_in_the_project_and_nowhere_else() {
        let (dir, outside) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let project = Project::open(dir.path()).unwrap();
        let sandbox = Sandbox::writing_in(vec![dir.path().to_owned()]);
        let cancel = CancellationToken::new();
        let call = Call { cancel: &cancel, approval: &Approval::Once, output: &|_| {} };
        let target = outside.path().join("escaped.txt");
        let command = format!("echo in > inside.txt && cat inside.txt && echo out > '{}'", target.display());
        let args = Args { command, timeout_ms: None, pending: false };
        let (text, outcome, confinement) = run(&project, &args, None, Some(&sandbox), &call).unwrap();
        if confinement == Confinement::Unavailable {
            eprintln!("Landlock is not available here; skipping");
            return;
        }
        assert_eq!(confinement, Confinement::Full);
        assert!(text.contains("in"), "{text}");
        assert!(text.to_ascii_lowercase().contains("permission denied"), "{text}");
        assert_ne!(outcome, Outcome::Exited(Some(0)));
        assert!(dir.path().join("inside.txt").exists() && !target.exists());
    }

    #[test]
    fn a_utf8_file_read_by_a_command_keeps_its_accents() {
        let dir = tempfile::tempdir().unwrap();
        // UTF-8 without a BOM, as every source file here: Windows PowerShell read it as ANSI.
        std::fs::write(dir.path().join("guía.md"), "# Krampsta — guía, ñandú\n").unwrap();
        let project = Project::open(dir.path()).unwrap();
        let cancel = CancellationToken::new();
        let call = Call { cancel: &cancel, approval: &Approval::Once, output: &|_| {} };
        let read = if cfg!(windows) { "Get-Content guía.md" } else { "cat guía.md" };
        let (text, outcome, _) =
            run(&project, &Args { command: read.into(), timeout_ms: None, pending: false }, None, None, &call).unwrap();
        assert_eq!(outcome, Outcome::Exited(Some(0)));
        assert!(text.contains("# Krampsta — guía, ñandú"), "{text}");
    }

    #[test]
    fn output_streams_and_comes_back_with_the_exit_code_from_the_project_folder() {
        let listing = if cfg!(windows) { "Get-ChildItem -Name; Write-Output 'héllo'" } else { "ls; echo héllo" };
        let (text, outcome, streamed) = run_it(listing, None, &CancellationToken::new());
        assert_eq!(outcome, Outcome::Exited(Some(0)));
        assert!(
            text.contains("marker.txt") && text.contains("héllo"),
            "runs in the project folder, UTF-8 intact: {text}"
        );
        assert_eq!(streamed, text.replace("\r\n", "\n"), "the user saw the same output, with plain line ends");

        // The same in both shells.
        assert_eq!(run_it("exit 3", None, &CancellationToken::new()).1, Outcome::Exited(Some(3)));
    }

    #[test]
    fn a_command_past_its_time_is_stopped_with_its_children() {
        let slow = if cfg!(windows) { "Start-Sleep -Seconds 30" } else { "sleep 30 & sleep 30" };
        let started = Instant::now();
        let (_, outcome, _) = run_it(slow, Some(1000), &CancellationToken::new());
        assert_eq!(outcome, Outcome::TimedOut(Duration::from_secs(1)));
        assert!(started.elapsed() < Duration::from_secs(10), "the whole tree was killed, the pipes closed");
    }

    #[test]
    fn a_cancelled_run_stops_the_command() {
        let cancel = CancellationToken::new();
        cancel.cancel();
        let slow = if cfg!(windows) { "Start-Sleep -Seconds 30" } else { "sleep 30" };
        let started = Instant::now();
        assert_eq!(run_it(slow, None, &cancel).1, Outcome::Stopped);
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    #[test]
    fn the_capture_keeps_head_and_tail_and_batches_never_split_a_character() {
        let mut c = Capture::default();
        c.push(&vec![b'a'; HEAD_BYTES]);
        c.push(&vec![b'b'; TAIL_BYTES * 2]);
        c.push(b"END");
        let text = c.into_text();
        assert!(text.starts_with(&"a".repeat(HEAD_BYTES)) && text.ends_with("END"));
        assert!(
            text.contains(&format!("({} bytes of output left out)", TAIL_BYTES + 3)),
            "{}",
            &text[HEAD_BYTES..HEAD_BYTES + 60]
        );

        let seen = Mutex::new(Vec::new());
        let out = |t: &str| seen.lock().unwrap().push(t.to_owned());
        let mut batch = Batch::default();
        let e = "é".as_bytes();
        batch.push(&[b'x', e[0]], &out);
        batch.flush(&out);
        batch.push(&[e[1]], &out);
        batch.flush(&out);
        assert_eq!(seen.into_inner().unwrap(), ["x", "é"]);
    }
}
