//! Server lifecycle: spawn the node launcher, supervise it, navigate the
//! embedded WebView when the harness is ready, and clean up the process tree
//! on exit.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{AppHandle, Manager, Url};

/// Port the harness serves on by default.
pub const PORT: u16 = 3080;
const ADDR: &str = "127.0.0.1:3080";
/// Long enough for a cold start: dependency install + full build + boot.
const READY_TIMEOUT: Duration = Duration::from_secs(600);

#[derive(Clone, Copy, PartialEq, Serialize)]
pub enum ServerPhase {
    Starting,
    Ready,
    Error,
}

/// Shared state exposed to the loading page through the `server_status` command.
pub struct ServerState {
    phase: Mutex<ServerPhase>,
    message: Mutex<String>,
    child: Mutex<Option<Child>>,
    stopping: Mutex<bool>,
}

#[derive(Serialize)]
pub struct ServerStatusInfo {
    pub phase: String,
    pub message: String,
}

impl Default for ServerState {
    fn default() -> Self {
        Self {
            phase: Mutex::new(ServerPhase::Starting),
            message: Mutex::new("正在初始化…".to_string()),
            child: Mutex::new(None),
            stopping: Mutex::new(false),
        }
    }
}

pub fn status(state: &ServerState) -> ServerStatusInfo {
    let phase = match *state.phase.lock().unwrap() {
        ServerPhase::Starting => "starting",
        ServerPhase::Ready => "ready",
        ServerPhase::Error => "error",
    };
    ServerStatusInfo {
        phase: phase.to_string(),
        message: state.message.lock().unwrap().clone(),
    }
}

fn set_message(state: &ServerState, msg: impl Into<String>) {
    *state.message.lock().unwrap() = msg.into();
}

fn set_phase(state: &ServerState, phase: ServerPhase, msg: &str) {
    *state.phase.lock().unwrap() = phase;
    *state.message.lock().unwrap() = msg.to_string();
}

fn set_error(state: &ServerState, msg: &str) {
    *state.phase.lock().unwrap() = ServerPhase::Error;
    *state.message.lock().unwrap() = msg.to_string();
}

// ------------------------------------------------------------------ discovery

/// Locate this client's root: env var, then relative to the running exe,
/// then relative to the current directory.
fn find_client_root() -> PathBuf {
    if let Ok(dir) = std::env::var("DSH_CLIENT_DIR") {
        if !dir.is_empty() {
            return PathBuf::from(dir);
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            for rel in ["..", "../..", "../../..", "../../../..", "../../../../.."] {
                let cand = dir.join(rel);
                if cand.join("scripts").join("dsh-server.mjs").exists() {
                    return cand;
                }
            }
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        for cand in [cwd.clone(), cwd.join("..")] {
            if cand.join("scripts").join("dsh-server.mjs").exists() {
                return cand;
            }
        }
    }
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

/// The deepseek-harness checkout: `DSH_REPO_ROOT` wins, else `<client>/../deepseek-harness`.
fn find_repo_root(client_root: &Path) -> PathBuf {
    if let Ok(dir) = std::env::var("DSH_REPO_ROOT") {
        let dir = PathBuf::from(dir);
        if dir.join("package.json").exists() {
            return dir;
        }
    }
    client_root.join("..").join("deepseek-harness")
}

// ------------------------------------------------------------------- startup

pub fn start(app: AppHandle) {
    let state = app.state::<ServerState>();
    let client_root = find_client_root();
    let repo_root = find_repo_root(&client_root);

    if !repo_root.join("package.json").exists() {
        set_error(
            &state,
            &format!(
                "未找到 deepseek-harness 仓库: {}\n可设置 DSH_REPO_ROOT 环境变量指向它。",
                repo_root.display()
            ),
        );
        return;
    }

    let script = client_root.join("scripts").join("dsh-server.mjs");
    let mut child = match Command::new("node")
        .arg(&script)
        .current_dir(&repo_root)
        .env("DSH_CLIENT_DIR", &client_root)
        .env("DSH_REPO_ROOT", &repo_root)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            set_error(&state, &format!("无法启动启动器 (node): {e}"));
            return;
        }
    };

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    *state.child.lock().unwrap() = Some(child);

    thread::spawn(move || supervise(app, stdout, stderr));
}

// -------------------------------------------------------------- supervision

fn supervise(
    app: AppHandle,
    stdout: Option<std::process::ChildStdout>,
    stderr: Option<std::process::ChildStderr>,
) {
    let state = app.state::<ServerState>();

    // Forward the launcher's [status]/[error] lines into shared state.
    // State is fetched inside each thread (State borrows the AppHandle, which
    // is moved in), so the closure stays 'static.
    if let Some(out) = stdout {
        let app2 = app.clone();
        thread::spawn(move || {
            let st = app2.state::<ServerState>();
            for line in BufReader::new(out).lines() {
                let Ok(line) = line else { break };
                handle_line(&st, &line);
            }
        });
    }
    if let Some(err) = stderr {
        let app2 = app.clone();
        thread::spawn(move || {
            let st = app2.state::<ServerState>();
            for line in BufReader::new(err).lines() {
                let Ok(line) = line else { break };
                handle_line(&st, &line);
            }
        });
    }

    let deadline = Instant::now() + READY_TIMEOUT;
    loop {
        if *state.stopping.lock().unwrap() {
            return;
        }

        // Did the launcher die before becoming ready?
        {
            let mut guard = state.child.lock().unwrap();
            if let Some(c) = guard.as_mut() {
                if let Some(status) = c.try_wait().unwrap_or(None) {
                    let already_error = *state.phase.lock().unwrap() == ServerPhase::Error;
                    drop(guard);
                    if !already_error {
                        let msg = match status.code() {
                            Some(code) => format!("服务器进程提前退出 (code {code})"),
                            None => "服务器进程被终止".to_string(),
                        };
                        set_error(&state, &msg);
                    }
                    return;
                }
            }
        }

        if http_ready() {
            set_phase(&state, ServerPhase::Ready, "服务器就绪");
            if let Some(win) = app.get_webview_window("main") {
                if let Ok(url) = Url::parse(&format!("http://{ADDR}")) {
                    let _ = win.navigate(url);
                }
            }
            // Stay alive (holding the child) until the app exits.
            loop {
                if *state.stopping.lock().unwrap() {
                    return;
                }
                let exited = {
                    let mut guard = state.child.lock().unwrap();
                    match guard.as_mut() {
                        Some(c) => c.try_wait().unwrap_or(None).is_some(),
                        None => true,
                    }
                };
                if exited {
                    return;
                }
                thread::sleep(Duration::from_millis(500));
            }
        }

        if Instant::now() >= deadline {
            set_error(&state, "等待服务器就绪超时");
            return;
        }
        thread::sleep(Duration::from_millis(500));
    }
}

fn handle_line(state: &ServerState, line: &str) {
    let line = line.trim();
    if let Some(rest) = line.strip_prefix("[status] ") {
        set_message(state, rest);
    } else if let Some(rest) = line.strip_prefix("[error] ") {
        set_error(state, rest);
    }
}

/// True once the harness answers an HTTP GET on the port. A plain TCP connect
/// can succeed against a socket that isn't actually serving, so read a response.
fn http_ready() -> bool {
    let addr = SocketAddr::from(([127, 0, 0, 1], PORT));
    let Ok(mut stream) = TcpStream::connect_timeout(&addr, Duration::from_millis(400)) else {
        return false;
    };
    let _ = stream.set_read_timeout(Some(Duration::from_millis(400)));
    let _ = stream.write_all(b"GET / HTTP/1.0\r\nHost: 127.0.0.1\r\n\r\n");
    let mut buf = [0u8; 32];
    match stream.read(&mut buf) {
        Ok(n) if n > 0 => {
            let text = String::from_utf8_lossy(&buf[..n]).to_lowercase();
            text.contains(" 200") || text.contains(" 200 ")
        }
        _ => false,
    }
}

// ----------------------------------------------------------------- shutdown

/// Kill the whole process tree (launcher → build/server children) and wait.
pub fn stop(app: &AppHandle) {
    let state = app.state::<ServerState>();
    *state.stopping.lock().unwrap() = true;
    let child = state.child.lock().unwrap().take();
    if let Some(mut child) = child {
        kill_tree(&mut child);
        let _ = child.wait();
    }
}

fn kill_tree(child: &mut Child) {
    #[cfg(windows)]
    {
        let pid = child.id();
        // taskkill /T kills the whole descendant tree (pnpm/node child servers).
        let _ = Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .output();
        let _ = child.kill();
    }
    #[cfg(not(windows))]
    {
        let _ = child.kill();
    }
}
