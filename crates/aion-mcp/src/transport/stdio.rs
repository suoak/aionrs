use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use async_trait::async_trait;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, BufWriter};
use tokio::process::{Child, ChildStdin, ChildStdout};
use tokio::sync::{Mutex, oneshot};

use super::{McpError, McpTransport};
use crate::protocol::{JsonRpcRequest, JsonRpcResponse};

type PendingResponses = Arc<Mutex<HashMap<u64, oneshot::Sender<Result<JsonRpcResponse, McpError>>>>>;

/// Stdio transport with one response reader and request-id routing.
pub struct StdioTransport {
    stdin: Mutex<BufWriter<ChildStdin>>,
    pending: PendingResponses,
    child: Mutex<Child>,
    reader_task: tokio::task::JoinHandle<()>,
    next_id: AtomicU64,
}

impl StdioTransport {
    /// Spawn a child process and return the transport.
    pub async fn spawn(command: &str, args: &[String], env: &HashMap<String, String>) -> Result<Self, McpError> {
        let mut cmd = tokio::process::Command::new(command);
        cmd.kill_on_drop(true)
            .args(args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::inherit())
            .envs(env);

        let mut child = cmd
            .spawn()
            .map_err(|error| McpError::Transport(format!("Failed to spawn '{command}': {error}")))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| McpError::Transport("Failed to capture child stdin".into()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| McpError::Transport("Failed to capture child stdout".into()))?;

        let pending = PendingResponses::default();
        let reader_pending = Arc::clone(&pending);
        let reader_task = tokio::spawn(async move {
            Self::route_responses(BufReader::new(stdout), reader_pending).await;
        });

        Ok(Self {
            stdin: Mutex::new(BufWriter::new(stdin)),
            pending,
            child: Mutex::new(child),
            reader_task,
            next_id: AtomicU64::new(1),
        })
    }

    /// Get the next request ID.
    pub fn next_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    async fn send(&self, request: &JsonRpcRequest) -> Result<(), McpError> {
        let json = serde_json::to_string(request)
            .map_err(|error| McpError::Transport(format!("JSON serialize error: {error}")))?;
        let mut stdin = self.stdin.lock().await;
        stdin
            .write_all(json.as_bytes())
            .await
            .map_err(|error| McpError::Transport(format!("Write to stdin failed: {error}")))?;
        stdin
            .write_all(b"\n")
            .await
            .map_err(|error| McpError::Transport(format!("Write newline failed: {error}")))?;
        stdin
            .flush()
            .await
            .map_err(|error| McpError::Transport(format!("Flush stdin failed: {error}")))
    }

    async fn route_responses(mut stdout: BufReader<ChildStdout>, pending: PendingResponses) {
        let mut line = String::new();
        loop {
            line.clear();
            let Ok(bytes_read) = stdout.read_line(&mut line).await else {
                break;
            };
            if bytes_read == 0 {
                break;
            }
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let Ok(response) = serde_json::from_str::<JsonRpcResponse>(trimmed) else {
                tracing::warn!(target: "aion_mcp", raw = %trimmed, "ignored malformed stdio JSON-RPC response");
                continue;
            };
            let Some(id) = response.id else {
                continue;
            };
            if let Some(sender) = pending.lock().await.remove(&id) {
                let result = if let Some(error) = &response.error {
                    Err(McpError::JsonRpc {
                        code: error.code,
                        message: error.message.clone(),
                    })
                } else {
                    Ok(response)
                };
                let _ = sender.send(result);
            }
        }

        for (_, sender) in pending.lock().await.drain() {
            let _ = sender.send(Err(McpError::Transport("MCP child process stdout closed".into())));
        }
    }
}

#[async_trait]
impl McpTransport for StdioTransport {
    async fn request(&self, request: &JsonRpcRequest) -> Result<JsonRpcResponse, McpError> {
        let id = request
            .id
            .ok_or_else(|| McpError::Transport("request id is required".into()))?;
        let (sender, receiver) = oneshot::channel();
        self.pending.lock().await.insert(id, sender);
        if let Err(error) = self.send(request).await {
            self.pending.lock().await.remove(&id);
            return Err(error);
        }
        receiver
            .await
            .map_err(|_| McpError::Transport("MCP response router stopped".into()))?
    }

    async fn notify(&self, request: &JsonRpcRequest) -> Result<(), McpError> {
        self.send(request).await
    }

    async fn close(&self) -> Result<(), McpError> {
        let mut child = self.child.lock().await;
        let _ = child.kill().await;
        self.reader_task.abort();
        Ok(())
    }
}
