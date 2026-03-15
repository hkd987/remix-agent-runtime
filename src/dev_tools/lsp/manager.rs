use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader, BufWriter};

use super::detection::resolve_server_command;
use super::types::{LspLanguage, LspServerHandle};
use crate::config::schema::LspConfig;
use crate::error::AgentError;

/// Manages LSP server instances — lazy-spawning them on first use.
pub struct LspServerManager {
    config: LspConfig,
    servers: HashMap<LspLanguage, LspServerHandle>,
    /// Languages that failed to start (to avoid retrying repeatedly).
    failed: HashMap<LspLanguage, String>,
}

impl LspServerManager {
    pub fn new(config: LspConfig) -> Self {
        Self {
            config,
            servers: HashMap::new(),
            failed: HashMap::new(),
        }
    }

    /// Get or lazily start an LSP server for the given language.
    pub async fn get_or_start(
        &mut self,
        language: LspLanguage,
    ) -> Result<&mut LspServerHandle, AgentError> {
        // Check if already failed
        if let Some(err) = self.failed.get(&language) {
            return Err(AgentError::ToolExecution(format!(
                "LSP server for {} previously failed to start: {}",
                language.language_id(),
                err
            )));
        }

        // If server exists, return it
        if self.servers.contains_key(&language) {
            return Ok(self.servers.get_mut(&language).unwrap());
        }

        // Try to start the server
        match self.start_server(language).await {
            Ok(handle) => {
                self.servers.insert(language, handle);
                Ok(self.servers.get_mut(&language).unwrap())
            }
            Err(e) => {
                let err_msg = e.to_string();
                self.failed.insert(language, err_msg);
                Err(e)
            }
        }
    }

    async fn start_server(&self, language: LspLanguage) -> Result<LspServerHandle, AgentError> {
        let (cmd, args) = resolve_server_command(language, &self.config).ok_or_else(|| {
            AgentError::ToolExecution(format!(
                "No LSP server found for {}. Install {} or configure --lsp-server {}=<command>",
                language.language_id(),
                language.default_server_command(),
                language.language_id()
            ))
        })?;

        tracing::info!(
            language = language.language_id(),
            command = %cmd,
            "Starting LSP server"
        );

        let mut process = tokio::process::Command::new(&cmd)
            .args(&args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| {
                AgentError::ToolExecution(format!("Failed to spawn LSP server '{}': {}", cmd, e))
            })?;

        let stdin = process.stdin.take().ok_or_else(|| {
            AgentError::ToolExecution("Failed to open LSP server stdin".to_string())
        })?;
        let stdout = process.stdout.take().ok_or_else(|| {
            AgentError::ToolExecution("Failed to open LSP server stdout".to_string())
        })?;

        let mut handle = LspServerHandle {
            language,
            process,
            stdin: BufWriter::new(stdin),
            stdout: BufReader::new(stdout),
            request_id: 0,
            initialized: false,
            diagnostics: HashMap::new(),
        };

        // Send initialize request
        self.send_initialize(&mut handle).await?;

        Ok(handle)
    }

    async fn send_initialize(&self, handle: &mut LspServerHandle) -> Result<(), AgentError> {
        let root_uri = format!(
            "file://{}",
            std::env::current_dir()
                .unwrap_or_default()
                .to_string_lossy()
        );

        let init_params = serde_json::json!({
            "processId": std::process::id(),
            "rootUri": root_uri,
            "capabilities": {
                "textDocument": {
                    "definition": { "dynamicRegistration": false },
                    "references": { "dynamicRegistration": false },
                    "publishDiagnostics": { "relatedInformation": true }
                }
            }
        });

        let _response = send_request(handle, "initialize", init_params).await?;

        // Send initialized notification
        send_notification(handle, "initialized", serde_json::json!({})).await?;

        handle.initialized = true;
        Ok(())
    }

    /// Shutdown all active LSP servers.
    pub fn shutdown_all(mut self) {
        for (lang, mut handle) in self.servers.drain() {
            tracing::debug!(language = lang.language_id(), "Shutting down LSP server");
            // Best-effort kill
            let _ = handle.process.start_kill();
        }
    }
}

impl std::fmt::Debug for LspServerManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LspServerManager")
            .field("active_servers", &self.servers.keys().collect::<Vec<_>>())
            .field("failed", &self.failed.keys().collect::<Vec<_>>())
            .finish()
    }
}

/// Send a JSON-RPC request and read the response.
pub async fn send_request(
    handle: &mut LspServerHandle,
    method: &str,
    params: serde_json::Value,
) -> Result<serde_json::Value, AgentError> {
    handle.request_id += 1;
    let id = handle.request_id;

    let request = serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": method,
        "params": params
    });

    let body = serde_json::to_string(&request)
        .map_err(|e| AgentError::ToolExecution(format!("Failed to serialize LSP request: {e}")))?;

    let message = format!("Content-Length: {}\r\n\r\n{}", body.len(), body);

    handle
        .stdin
        .write_all(message.as_bytes())
        .await
        .map_err(|e| AgentError::ToolExecution(format!("Failed to write to LSP: {e}")))?;
    handle
        .stdin
        .flush()
        .await
        .map_err(|e| AgentError::ToolExecution(format!("Failed to flush LSP stdin: {e}")))?;

    // Read response (may receive notifications before the response)
    loop {
        let response = read_message(&mut handle.stdout).await?;

        // Check if this is a notification (no id field)
        if response.get("id").is_none() {
            // Handle notifications like publishDiagnostics
            if let Some(method) = response.get("method").and_then(|m| m.as_str()) {
                if method == "textDocument/publishDiagnostics" {
                    if let Some(params) = response.get("params") {
                        if let Some(uri) = params.get("uri").and_then(|u| u.as_str()) {
                            if let Some(diags) = params.get("diagnostics") {
                                if let Ok(diagnostics) =
                                    serde_json::from_value::<Vec<lsp_types::Diagnostic>>(
                                        diags.clone(),
                                    )
                                {
                                    handle.diagnostics.insert(uri.to_string(), diagnostics);
                                }
                            }
                        }
                    }
                }
            }
            continue;
        }

        // Check if this is our response
        if let Some(resp_id) = response.get("id").and_then(|i| i.as_i64()) {
            if resp_id == id {
                if let Some(error) = response.get("error") {
                    return Err(AgentError::ToolExecution(format!("LSP error: {}", error)));
                }
                return Ok(response
                    .get("result")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null));
            }
        }
    }
}

/// Send a JSON-RPC notification (no response expected).
pub async fn send_notification(
    handle: &mut LspServerHandle,
    method: &str,
    params: serde_json::Value,
) -> Result<(), AgentError> {
    let notification = serde_json::json!({
        "jsonrpc": "2.0",
        "method": method,
        "params": params
    });

    let body = serde_json::to_string(&notification).map_err(|e| {
        AgentError::ToolExecution(format!("Failed to serialize LSP notification: {e}"))
    })?;

    let message = format!("Content-Length: {}\r\n\r\n{}", body.len(), body);

    handle
        .stdin
        .write_all(message.as_bytes())
        .await
        .map_err(|e| AgentError::ToolExecution(format!("Failed to write to LSP: {e}")))?;
    handle
        .stdin
        .flush()
        .await
        .map_err(|e| AgentError::ToolExecution(format!("Failed to flush LSP stdin: {e}")))?;

    Ok(())
}

/// Read a single LSP base protocol message from stdout.
async fn read_message(
    reader: &mut BufReader<tokio::process::ChildStdout>,
) -> Result<serde_json::Value, AgentError> {
    let mut content_length: usize = 0;

    // Read headers
    loop {
        let mut line = String::new();
        reader
            .read_line(&mut line)
            .await
            .map_err(|e| AgentError::ToolExecution(format!("Failed to read LSP header: {e}")))?;

        let trimmed = line.trim();
        if trimmed.is_empty() {
            break;
        }

        if let Some(len_str) = trimmed.strip_prefix("Content-Length: ") {
            content_length = len_str
                .parse()
                .map_err(|e| AgentError::ToolExecution(format!("Invalid Content-Length: {e}")))?;
        }
    }

    if content_length == 0 {
        return Err(AgentError::ToolExecution(
            "LSP message with zero Content-Length".to_string(),
        ));
    }

    // Read body
    let mut body = vec![0u8; content_length];
    reader
        .read_exact(&mut body)
        .await
        .map_err(|e| AgentError::ToolExecution(format!("Failed to read LSP body: {e}")))?;

    serde_json::from_slice(&body)
        .map_err(|e| AgentError::ToolExecution(format!("Failed to parse LSP response: {e}")))
}

/// Resolve a file path to an absolute path.
pub fn resolve_path(file: &str) -> Result<PathBuf, AgentError> {
    let path = Path::new(file);
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        let cwd = std::env::current_dir().map_err(|e| {
            AgentError::ToolExecution(format!("Failed to get current directory: {e}"))
        })?;
        Ok(cwd.join(path))
    }
}

/// Open a document in the LSP server: reads the file, sends `textDocument/didOpen`,
/// and returns the file URI.
pub async fn open_document(
    handle: &mut LspServerHandle,
    file_path: &Path,
    language: LspLanguage,
) -> Result<String, AgentError> {
    let file_uri = format!("file://{}", file_path.to_string_lossy());

    let content = tokio::fs::read_to_string(file_path).await.map_err(|e| {
        AgentError::ToolExecution(format!(
            "Failed to read file {}: {}",
            file_path.display(),
            e
        ))
    })?;

    send_notification(
        handle,
        "textDocument/didOpen",
        serde_json::json!({
            "textDocument": {
                "uri": file_uri,
                "languageId": language.language_id(),
                "version": 1,
                "text": content
            }
        }),
    )
    .await?;

    Ok(file_uri)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_manager() {
        let config = LspConfig::default();
        let manager = LspServerManager::new(config);
        assert!(manager.servers.is_empty());
        assert!(manager.failed.is_empty());
    }

    #[test]
    fn test_debug_impl() {
        let config = LspConfig::default();
        let manager = LspServerManager::new(config);
        let debug = format!("{:?}", manager);
        assert!(debug.contains("LspServerManager"));
    }

    #[test]
    fn test_resolve_path_absolute() {
        let result = resolve_path("/absolute/path.rs").unwrap();
        assert_eq!(result, std::path::PathBuf::from("/absolute/path.rs"));
    }

    #[test]
    fn test_resolve_path_relative() {
        let result = resolve_path("src/main.rs").unwrap();
        assert!(result.is_absolute());
        assert!(result.to_string_lossy().ends_with("src/main.rs"));
    }
}
