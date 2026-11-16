use chrono::Utc;
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use serde::Serialize;
use std::collections::{HashMap, VecDeque};
use std::io::{Read, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter};
use uuid::Uuid;

const MAX_OUTPUT_BYTES: usize = 512 * 1024;
const MAX_OUTPUT_CHUNKS: usize = 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalState {
    Starting,
    Ready,
    Running,
    Interrupting,
    Closed,
    Exited,
    Failed,
}

#[derive(Clone, Serialize, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TerminalOutputChunk {
    pub sequence: u64,
    pub data: String,
    pub at: String,
}

#[derive(Default)]
struct OutputBuffer {
    chunks: VecDeque<TerminalOutputChunk>,
    bytes: usize,
    next_sequence: u64,
}

impl OutputBuffer {
    fn push(&mut self, data: String) -> TerminalOutputChunk {
        let chunk = TerminalOutputChunk {
            sequence: self.next_sequence,
            data,
            at: Utc::now().to_rfc3339(),
        };
        self.next_sequence += 1;
        self.bytes += chunk.data.len();
        self.chunks.push_back(chunk.clone());
        while self.bytes > MAX_OUTPUT_BYTES || self.chunks.len() > MAX_OUTPUT_CHUNKS {
            if let Some(removed) = self.chunks.pop_front() {
                self.bytes = self.bytes.saturating_sub(removed.data.len());
            } else {
                break;
            }
        }
        chunk
    }

    fn read(&self, after_sequence: Option<u64>, limit: usize) -> (Vec<TerminalOutputChunk>, bool) {
        let first = self.chunks.front().map(|chunk| chunk.sequence);
        let truncated = matches!((after_sequence, first), (Some(after), Some(first)) if after.saturating_add(1) < first);
        let chunks = self
            .chunks
            .iter()
            .filter(|chunk| {
                after_sequence
                    .map(|after| chunk.sequence > after)
                    .unwrap_or(true)
            })
            .take(limit.clamp(1, 512))
            .cloned()
            .collect();
        (chunks, truncated)
    }
}

struct TerminalSession {
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn Child + Send + Sync>,
    state: TerminalState,
    output: OutputBuffer,
    active_command_id: Option<String>,
    active_command: Option<String>,
    repl_mode: bool,
    pending_repl: bool,
    prompt_tail: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalStarted {
    pub session_id: String,
    pub next_sequence: u64,
    pub state: TerminalState,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalCommandAccepted {
    pub command_id: String,
    pub state: TerminalState,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalInterruptResult {
    pub command_id: Option<String>,
    pub state: TerminalState,
    pub signal_sent: bool,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalReadResult {
    pub session_id: String,
    pub chunks: Vec<TerminalOutputChunk>,
    pub next_sequence: u64,
    pub truncated: bool,
    pub state: TerminalState,
    pub active_command_id: Option<String>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct TerminalOutput {
    session_id: String,
    sequence: u64,
    data: String,
    at: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct TerminalStatus {
    session_id: String,
    status: TerminalState,
    exit_code: Option<u32>,
    command_id: Option<String>,
}

#[derive(Default)]
pub struct TerminalManager {
    sessions: Arc<Mutex<HashMap<String, TerminalSession>>>,
}

impl TerminalManager {
    pub fn start(
        &self,
        app: AppHandle,
        shell: &str,
        cwd: &Path,
        rows: u16,
        cols: u16,
    ) -> Result<TerminalStarted, String> {
        let shell = validated_shell(shell)?;
        if !cwd.is_dir() {
            return Err("终端工作目录不存在".into());
        }
        let pair = native_pty_system()
            .openpty(PtySize {
                rows: rows.clamp(8, 200),
                cols: cols.clamp(20, 400),
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|error| error.to_string())?;
        let mut command = CommandBuilder::new(&shell);
        command.cwd(cwd);
        let lower = shell.to_ascii_lowercase();
        if lower.contains("powershell") || lower.ends_with("pwsh.exe") || lower == "pwsh" {
            command.args(["-NoLogo", "-NoProfile"]);
        } else {
            command.args(["/D", "/Q"]);
        }
        let child = pair
            .slave
            .spawn_command(command)
            .map_err(|error| error.to_string())?;
        drop(pair.slave);
        let mut reader = pair
            .master
            .try_clone_reader()
            .map_err(|error| error.to_string())?;
        let writer = pair
            .master
            .take_writer()
            .map_err(|error| error.to_string())?;
        let session_id = format!("terminal-{}", Uuid::new_v4());
        self.sessions
            .lock()
            .map_err(|_| "terminal session lock was poisoned".to_string())?
            .insert(
                session_id.clone(),
                TerminalSession {
                    master: pair.master,
                    writer,
                    child,
                    state: TerminalState::Starting,
                    output: OutputBuffer::default(),
                    active_command_id: None,
                    active_command: None,
                    repl_mode: false,
                    pending_repl: false,
                    prompt_tail: String::new(),
                },
            );
        let sessions = Arc::clone(&self.sessions);
        let reader_session_id = session_id.clone();
        std::thread::spawn(move || {
            let mut buffer = [0_u8; 4096];
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(length) => {
                        let data = String::from_utf8_lossy(&buffer[..length]).to_string();
                        let (chunk, ready_command) = match sessions.lock() {
                            Ok(mut guard) => match guard.get_mut(&reader_session_id) {
                                Some(session) => {
                                    // PowerShell asks the terminal for its cursor position while
                                    // starting. A GUI terminal emulator answers this DSR query,
                                    // but a raw PTY client must do so itself; otherwise PowerShell
                                    // waits forever before printing its first prompt.
                                    for response in terminal_query_responses(&data) {
                                        let _ = session.writer.write_all(response);
                                        let _ = session.writer.flush();
                                    }
                                    session.prompt_tail.push_str(&strip_ansi_for_prompt(&data));
                                    if session.prompt_tail.chars().count() > 512 {
                                        session.prompt_tail = session
                                            .prompt_tail
                                            .chars()
                                            .rev()
                                            .take(512)
                                            .collect::<String>()
                                            .chars()
                                            .rev()
                                            .collect();
                                    }
                                    let active_finished =
                                        session.active_command.as_ref().is_some_and(|command| {
                                            command_followed_by_prompt(
                                                &session.prompt_tail,
                                                command,
                                                session.repl_mode || session.pending_repl,
                                            )
                                        });
                                    let interrupted = session.state == TerminalState::Interrupting
                                        && contains_any_prompt(&session.prompt_tail);
                                    let initial_ready = session.active_command_id.is_none()
                                        && session.state == TerminalState::Starting
                                        && contains_primary_shell_prompt(&session.prompt_tail);
                                    let ready_command = if active_finished
                                        || interrupted
                                        || initial_ready
                                    {
                                        if session.pending_repl
                                            && contains_python_prompt(&session.prompt_tail)
                                        {
                                            session.repl_mode = true;
                                        } else if session.repl_mode
                                            && contains_primary_shell_prompt(&session.prompt_tail)
                                        {
                                            session.repl_mode = false;
                                        }
                                        session.pending_repl = false;
                                        session.active_command = None;
                                        session.state = TerminalState::Ready;
                                        session.active_command_id.take()
                                    } else {
                                        None
                                    };
                                    (session.output.push(data), ready_command)
                                }
                                None => break,
                            },
                            Err(_) => break,
                        };
                        let _ = app.emit(
                            "terminal://output",
                            TerminalOutput {
                                session_id: reader_session_id.clone(),
                                sequence: chunk.sequence,
                                data: chunk.data,
                                at: chunk.at,
                            },
                        );
                        if ready_command.is_some() {
                            let _ = app.emit(
                                "terminal://status",
                                TerminalStatus {
                                    session_id: reader_session_id.clone(),
                                    status: TerminalState::Ready,
                                    exit_code: None,
                                    command_id: ready_command,
                                },
                            );
                        }
                    }
                    Err(_) => break,
                }
            }
            let (exit_code, command_id) = sessions
                .lock()
                .ok()
                .and_then(|mut guard| {
                    guard.get_mut(&reader_session_id).map(|session| {
                        session.state = TerminalState::Exited;
                        let command_id = session.active_command_id.take();
                        let exit_code = session
                            .child
                            .try_wait()
                            .ok()
                            .flatten()
                            .map(|status| status.exit_code());
                        (exit_code, command_id)
                    })
                })
                .unwrap_or((None, None));
            let _ = app.emit(
                "terminal://status",
                TerminalStatus {
                    session_id: reader_session_id,
                    status: TerminalState::Exited,
                    exit_code,
                    command_id,
                },
            );
        });
        Ok(TerminalStarted {
            session_id,
            next_sequence: 0,
            state: TerminalState::Starting,
        })
    }

    pub fn write_command(
        &self,
        session_id: &str,
        command: &str,
    ) -> Result<TerminalCommandAccepted, String> {
        validate_terminal_command(command)?;
        let mut guard = self
            .sessions
            .lock()
            .map_err(|_| "terminal session lock was poisoned".to_string())?;
        let session = guard
            .get_mut(session_id)
            .ok_or_else(|| "终端会话不存在或已经结束".to_string())?;
        if matches!(
            session.state,
            TerminalState::Running | TerminalState::Interrupting
        ) {
            return Err("当前命令仍在运行，请等待完成或先停止".into());
        }
        if matches!(
            session.state,
            TerminalState::Exited | TerminalState::Closed | TerminalState::Failed
        ) {
            return Err("终端会话已经结束，请重新连接".into());
        }
        let command_id = format!("command-{}", Uuid::new_v4());
        session
            .writer
            .write_all(command.as_bytes())
            .and_then(|_| session.writer.write_all(b"\r\n"))
            .and_then(|_| session.writer.flush())
            .map_err(|error| error.to_string())?;
        session.active_command_id = Some(command_id.clone());
        session.active_command = Some(command.trim().to_string());
        session.pending_repl = !session.repl_mode && is_python_repl_launch(command);
        session.state = TerminalState::Running;
        session.prompt_tail.clear();
        Ok(TerminalCommandAccepted {
            command_id,
            state: TerminalState::Running,
        })
    }

    pub fn read(
        &self,
        session_id: &str,
        after_sequence: Option<u64>,
        limit: usize,
    ) -> Result<TerminalReadResult, String> {
        let guard = self
            .sessions
            .lock()
            .map_err(|_| "terminal session lock was poisoned".to_string())?;
        let session = guard
            .get(session_id)
            .ok_or_else(|| "终端会话不存在或已经结束".to_string())?;
        let (chunks, truncated) = session.output.read(after_sequence, limit);
        Ok(TerminalReadResult {
            session_id: session_id.to_string(),
            chunks,
            next_sequence: session.output.next_sequence,
            truncated,
            state: session.state,
            active_command_id: session.active_command_id.clone(),
        })
    }

    pub fn interrupt(&self, session_id: &str) -> Result<TerminalInterruptResult, String> {
        let mut guard = self
            .sessions
            .lock()
            .map_err(|_| "terminal session lock was poisoned".to_string())?;
        let session = guard
            .get_mut(session_id)
            .ok_or_else(|| "终端会话不存在或已经结束".to_string())?;
        let command_id = session.active_command_id.clone();
        if command_id.is_none() || session.state == TerminalState::Ready {
            return Ok(TerminalInterruptResult {
                command_id: None,
                state: session.state,
                signal_sent: false,
            });
        }
        session
            .writer
            .write_all(&[3])
            .and_then(|_| session.writer.flush())
            .map_err(|error| error.to_string())?;
        session.state = TerminalState::Interrupting;
        session.prompt_tail.clear();
        Ok(TerminalInterruptResult {
            command_id,
            state: TerminalState::Interrupting,
            signal_sent: true,
        })
    }

    pub fn resize(&self, session_id: &str, rows: u16, cols: u16) -> Result<(), String> {
        let guard = self
            .sessions
            .lock()
            .map_err(|_| "terminal session lock was poisoned".to_string())?;
        let session = guard
            .get(session_id)
            .ok_or_else(|| "终端会话不存在或已经结束".to_string())?;
        session
            .master
            .resize(PtySize {
                rows: rows.clamp(8, 200),
                cols: cols.clamp(20, 400),
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|error| error.to_string())
    }

    pub fn close(&self, session_id: &str) -> Result<(), String> {
        let mut session = self
            .sessions
            .lock()
            .map_err(|_| "terminal session lock was poisoned".to_string())?
            .remove(session_id)
            .ok_or_else(|| "终端会话不存在或已经结束".to_string())?;
        session.state = TerminalState::Closed;
        session.child.kill().map_err(|error| error.to_string())
    }
}

impl Drop for TerminalManager {
    fn drop(&mut self) {
        if let Ok(mut sessions) = self.sessions.lock() {
            for (_, mut session) in sessions.drain() {
                let _ = session.child.kill();
            }
        }
    }
}

fn validated_shell(shell: &str) -> Result<String, String> {
    let value = shell.trim();
    let name = Path::new(value)
        .file_name()
        .and_then(|part| part.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if ![
        "powershell",
        "powershell.exe",
        "pwsh",
        "pwsh.exe",
        "cmd",
        "cmd.exe",
    ]
    .contains(&name.as_str())
    {
        return Err("只支持 PowerShell、pwsh 或 cmd 作为交互式 Shell".into());
    }
    Ok(value.to_string())
}

pub fn validate_terminal_command(command: &str) -> Result<(), String> {
    let value = command.trim();
    if value.is_empty() {
        return Err("命令不能为空".into());
    }
    if value.len() > 32_000 {
        return Err("命令过长".into());
    }
    if value.contains("..")
        || value.contains(":\\")
        || value.contains(":/")
        || value.contains("\\\\")
    {
        return Err("命令包含明显的工作区外路径，已拒绝".into());
    }
    Ok(())
}

fn strip_ansi_for_prompt(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    let mut chars = value.chars().peekable();
    while let Some(character) = chars.next() {
        if character == '\u{1b}' && chars.peek() == Some(&'[') {
            chars.next();
            for next in chars.by_ref() {
                if ('@'..='~').contains(&next) {
                    break;
                }
            }
        } else {
            output.push(character);
        }
    }
    output
}

fn terminal_query_responses(value: &str) -> Vec<&'static [u8]> {
    let mut responses = Vec::new();
    if value.contains("\u{1b}[5n") {
        responses.push(b"\x1b[0n".as_slice());
    }
    if value.contains("\u{1b}[6n") {
        responses.push(b"\x1b[1;1R".as_slice());
    }
    responses
}

fn looks_like_prompt(value: &str) -> bool {
    let tail = value.trim_end_matches(['\r', '\n']);
    let line = tail
        .rsplit(['\r', '\n'])
        .next()
        .unwrap_or(tail)
        .trim_start();
    let trimmed = line.trim_end();
    trimmed == ">>>"
        || trimmed == "..."
        || (trimmed.starts_with("PS ") && trimmed.ends_with('>'))
        || (trimmed.ends_with('>') && (trimmed.contains(":\\") || trimmed.contains(":/")))
}

fn contains_primary_shell_prompt(value: &str) -> bool {
    value.lines().any(|line| {
        let trimmed = line.trim();
        (trimmed.starts_with("PS ") && trimmed.ends_with('>'))
            || (trimmed.ends_with('>') && (trimmed.contains(":\\") || trimmed.contains(":/")))
    })
}

fn contains_python_prompt(value: &str) -> bool {
    value
        .lines()
        .any(|line| matches!(line.trim(), ">>>" | "..."))
}

fn contains_any_prompt(value: &str) -> bool {
    looks_like_prompt(value)
        || contains_primary_shell_prompt(value)
        || contains_python_prompt(value)
}

fn command_followed_by_prompt(value: &str, command: &str, allow_repl: bool) -> bool {
    if let Some(position) = value.rfind(command) {
        let suffix = &value[position + command.len()..];
        if contains_primary_shell_prompt(suffix) || (allow_repl && contains_python_prompt(suffix)) {
            return true;
        }
    }
    // ConPTY redraws a wrapped input line as several chunks. The transcript
    // can therefore contain `print(probe_valu\r\ne)` even though the REPL
    // received `print(probe_value)`. Compare a control/whitespace-free view,
    // then require a *new* primary prompt after that echoed command.
    let compact_value = compact_terminal_text(value);
    let compact_command = compact_terminal_text(command);
    let Some(position) = compact_value.rfind(&compact_command) else {
        return false;
    };
    let suffix = &compact_value[position + compact_command.len()..];
    let shell_prompt = suffix.split("PS").skip(1).any(|part| part.contains('>'))
        || (suffix.contains(":\\") && suffix.contains('>'));
    shell_prompt || (allow_repl && (suffix.contains(">>>") || suffix.contains("...")))
}

fn compact_terminal_text(value: &str) -> String {
    value
        .chars()
        .filter(|character| !character.is_whitespace() && !character.is_control())
        .collect()
}

fn is_python_repl_launch(command: &str) -> bool {
    let normalized = command.trim().to_ascii_lowercase();
    matches!(
        normalized.as_str(),
        "python" | "python.exe" | "py" | "py -3" | "py -3.11" | "py -3.12" | "py -3.13"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_commands_reject_obvious_escape_paths() {
        assert!(validate_terminal_command("Get-ChildItem src").is_ok());
        assert!(validate_terminal_command("Get-Content ..\\secret.txt").is_err());
        assert!(validate_terminal_command("Get-Content C:\\secret.txt").is_err());
        assert!(validated_shell("powershell.exe").is_ok());
        assert!(validated_shell("python.exe").is_err());
    }

    #[test]
    fn output_buffer_replays_only_missing_sequences() {
        let mut buffer = OutputBuffer::default();
        assert_eq!(buffer.push("one".into()).sequence, 0);
        assert_eq!(buffer.push("two".into()).sequence, 1);
        assert_eq!(buffer.push("three".into()).sequence, 2);
        let (chunks, truncated) = buffer.read(Some(0), 20);
        assert!(!truncated);
        assert_eq!(
            chunks
                .iter()
                .map(|chunk| chunk.sequence)
                .collect::<Vec<_>>(),
            vec![1, 2]
        );
    }

    #[test]
    fn common_shell_and_repl_prompts_are_detected() {
        assert!(looks_like_prompt("PS E:\\project>"));
        assert!(looks_like_prompt("C:\\project> "));
        assert!(looks_like_prompt(">>> "));
        assert!(!looks_like_prompt(">> "));
        assert!(!looks_like_prompt(
            "PS E:\\project> Start-Sleep -Seconds 20"
        ));
        assert!(!looks_like_prompt("still running\r\n"));
        assert!(command_followed_by_prompt(
            "PS E:\\project> Start-Sleep -Seconds 20\r\nfinished\r\nPS E:\\project> ",
            "Start-Sleep -Seconds 20",
            false,
        ));
        assert!(!command_followed_by_prompt(
            "PS E:\\project> Start-Sleep -Seconds 20\r\n>> ",
            "Start-Sleep -Seconds 20",
            false,
        ));
        assert!(command_followed_by_prompt(
            "PS E:\\project> python\r\n>>> ",
            "python",
            true
        ));
        assert!(command_followed_by_prompt(
            ">>> print(probe_valu\r\ne)\r\n42\r\n>>> ",
            "print(probe_value)",
            true
        ));
        assert!(is_python_repl_launch("python"));
    }

    #[test]
    fn terminal_status_queries_receive_emulator_responses() {
        assert_eq!(
            terminal_query_responses("\u{1b}[5n"),
            vec![b"\x1b[0n".as_slice()]
        );
        assert_eq!(
            terminal_query_responses("\u{1b}[6n"),
            vec![b"\x1b[1;1R".as_slice()]
        );
    }
}
