use serde_json::{json, Value};
use std::{
    env,
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command, Stdio},
    sync::{
        mpsc::{self, Receiver},
        Arc, Mutex,
    },
    thread,
    time::Duration,
};

const RESPONSE_TIMEOUT: Duration = Duration::from_secs(8);

#[derive(Clone, Default)]
pub struct CodexAppServer {
    inner: Arc<Mutex<Option<CodexRpc>>>,
}

impl CodexAppServer {
    pub fn request(&self, method: &str) -> Result<Value, String> {
        let mut guard = self
            .inner
            .lock()
            .map_err(|_| "Codex app-server state is unavailable".to_string())?;

        for attempt in 0..2 {
            if guard.is_none() {
                *guard = Some(CodexRpc::start()?);
            }

            let result = guard
                .as_mut()
                .expect("Codex app-server was initialized")
                .request(method);

            match result {
                Ok(value) => return Ok(value),
                Err(error) if attempt == 0 => {
                    if let Some(mut rpc) = guard.take() {
                        rpc.stop();
                    }
                    let _ = error;
                }
                Err(error) => return Err(error),
            }
        }

        Err("Codex app-server request failed".to_string())
    }
}

struct CodexRpc {
    child: Child,
    stdin: ChildStdin,
    lines: Receiver<String>,
    stderr: Arc<Mutex<Vec<String>>>,
    next_id: u64,
}

impl CodexRpc {
    fn start() -> Result<Self, String> {
        let executable = resolve_codex_executable()
            .ok_or_else(|| "Codex CLI was not found on this Mac".to_string())?;
        let mut child = Command::new(&executable)
            .args(["app-server", "--stdio"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| {
                format!(
                    "Could not start Codex app-server at {}: {error}",
                    executable.display()
                )
            })?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| "Codex app-server stdin was unavailable".to_string())?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "Codex app-server stdout was unavailable".to_string())?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| "Codex app-server stderr was unavailable".to_string())?;
        let (sender, lines) = mpsc::channel();
        let stderr_lines = Arc::new(Mutex::new(Vec::new()));
        let stderr_output = stderr_lines.clone();

        thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                match line {
                    Ok(line) => {
                        if sender.send(line).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });
        thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                if let Ok(mut output) = stderr_output.lock() {
                    output.push(line);
                    if output.len() > 8 {
                        output.remove(0);
                    }
                }
            }
        });

        let mut rpc = Self {
            child,
            stdin,
            lines,
            stderr: stderr_lines,
            next_id: 1,
        };
        let initialize_id = rpc.next_request_id();
        rpc.write_message(&json!({
            "id": initialize_id,
            "method": "initialize",
            "params": {
                "clientInfo": {
                    "name": "quotaglass",
                    "title": "QuotaGlass",
                    "version": env!("CARGO_PKG_VERSION")
                },
                "capabilities": {
                    "experimentalApi": false
                }
            }
        }))?;
        rpc.wait_for_response(initialize_id)?;
        rpc.write_message(&json!({"method": "initialized"}))?;
        Ok(rpc)
    }

    fn request(&mut self, method: &str) -> Result<Value, String> {
        let id = self.next_request_id();
        self.write_message(&json!({
            "id": id,
            "method": method,
            "params": null
        }))?;
        self.wait_for_response(id)
    }

    fn next_request_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    fn write_message(&mut self, message: &Value) -> Result<(), String> {
        serde_json::to_writer(&mut self.stdin, message)
            .map_err(|error| format!("Could not encode Codex request: {error}"))?;
        self.stdin
            .write_all(b"\n")
            .and_then(|_| self.stdin.flush())
            .map_err(|error| format!("Could not send Codex request: {error}"))
    }

    fn wait_for_response(&self, id: u64) -> Result<Value, String> {
        loop {
            let line = self.lines.recv_timeout(RESPONSE_TIMEOUT).map_err(|error| {
                let detail = self
                    .stderr
                    .lock()
                    .ok()
                    .map(|lines| lines.join(" "))
                    .filter(|text| !text.is_empty())
                    .map(|text| format!(" ({text})"))
                    .unwrap_or_default();
                format!("Codex app-server did not respond: {error}{detail}")
            })?;
            let message: Value = match serde_json::from_str(&line) {
                Ok(message) => message,
                Err(_) => continue,
            };
            if response_id(&message) != Some(id) {
                continue;
            }
            if let Some(error) = message.get("error") {
                return Err(format!("Codex app-server returned an error: {error}"));
            }
            return message
                .get("result")
                .cloned()
                .ok_or_else(|| "Codex app-server response had no result".to_string());
        }
    }

    fn stop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for CodexRpc {
    fn drop(&mut self) {
        self.stop();
    }
}

fn response_id(message: &Value) -> Option<u64> {
    message.get("id")?.as_u64()
}

fn resolve_codex_executable() -> Option<PathBuf> {
    let candidates = [
        env::var_os("CODEX_BIN").map(PathBuf::from),
        Some(PathBuf::from("/opt/homebrew/bin/codex")),
        Some(PathBuf::from("/usr/local/bin/codex")),
        Some(PathBuf::from(
            "/Applications/ChatGPT.app/Contents/Resources/codex",
        )),
    ];

    candidates
        .into_iter()
        .flatten()
        .find(|candidate| is_executable(candidate))
        .or_else(|| Some(PathBuf::from("codex")))
}

fn is_executable(path: &Path) -> bool {
    path.is_file()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn response_id_ignores_notifications() {
        assert_eq!(response_id(&json!({"method": "account/updated"})), None);
        assert_eq!(response_id(&json!({"id": 4, "result": {}})), Some(4));
    }

    #[test]
    #[ignore = "requires a locally installed and authenticated Codex CLI"]
    fn live_codex_usage_methods_respond() {
        let server = CodexAppServer::default();
        let limits = server.request("account/rateLimits/read").unwrap();
        assert!(limits.get("rateLimits").is_some());
        let usage = server.request("account/usage/read").unwrap();
        assert!(usage.get("summary").is_some());
    }
}
