use std::{
    io::{BufRead, BufReader, Write as _},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command, Stdio},
    sync::{Arc, Mutex, mpsc},
    thread,
};

use anyhow::{Context as _, Result};
use serde_json::{Value, json};
use url::Url;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LspDiagnostic {
    pub path: PathBuf,
    pub start_line: usize,
    pub start_character: usize,
    pub end_line: usize,
    pub end_character: usize,
    pub severity: u64,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DefinitionLocation {
    pub path: PathBuf,
    pub start_line: usize,
    pub start_character: usize,
    pub end_line: usize,
    pub end_character: usize,
}

#[derive(Debug)]
pub enum LspEvent {
    Diagnostics {
        path: PathBuf,
        items: Vec<LspDiagnostic>,
    },
    Response {
        id: u64,
        result: Value,
    },
    ServerRequest {
        id: u64,
        method: String,
        params: Value,
    },
    Failed(String),
}

pub struct LspClient {
    child: Child,
    stdin: Arc<Mutex<ChildStdin>>,
    events: mpsc::Receiver<LspEvent>,
    next_id: u64,
    initialize_id: u64,
    initialized: bool,
}

impl LspClient {
    pub fn start(command: &[String], root: &Path) -> Result<Self> {
        let (program, arguments) = command
            .split_first()
            .context("LSP command must include an executable")?;
        let mut child = Command::new(program)
            .args(arguments)
            .current_dir(root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .with_context(|| format!("could not start language server {program}"))?;
        let stdin = Arc::new(Mutex::new(child.stdin.take().context("missing LSP stdin")?));
        let stdout = child.stdout.take().context("missing LSP stdout")?;
        let (sender, events) = mpsc::channel();
        thread::Builder::new()
            .name("textify-lsp-reader".to_owned())
            .spawn(move || read_server_messages(stdout, sender))
            .context("could not start LSP reader")?;

        let mut client = Self {
            child,
            stdin,
            events,
            next_id: 1,
            initialize_id: 0,
            initialized: false,
        };
        let root_uri = path_to_file_uri(root)?;
        let initialize_id = client.request(
            "initialize",
            json!({
                "processId": std::process::id(),
                "rootUri": root_uri,
                "capabilities": {
                    "textDocument": {
                        "publishDiagnostics": {"relatedInformation": false},
                        "definition": {"linkSupport": true}
                    }
                },
                "clientInfo": {"name": "Textify", "version": env!("CARGO_PKG_VERSION")}
            }),
        )?;
        client.initialize_id = initialize_id;
        Ok(client)
    }

    pub fn initialize_id(&self) -> u64 {
        self.initialize_id
    }

    pub fn is_initialized(&self) -> bool {
        self.initialized
    }

    pub fn finish_initialize(&mut self) -> Result<()> {
        if !self.initialized {
            self.notify("initialized", json!({}))?;
            self.initialized = true;
        }
        Ok(())
    }

    pub fn did_open(&self, path: &Path, language_id: &str, version: u64, text: &str) -> Result<()> {
        self.notify(
            "textDocument/didOpen",
            json!({
                "textDocument": {
                    "uri": path_to_file_uri(path)?,
                    "languageId": language_id,
                    "version": version,
                    "text": text
                }
            }),
        )
    }

    pub fn did_change(&self, path: &Path, version: u64, text: &str) -> Result<()> {
        self.notify(
            "textDocument/didChange",
            json!({
                "textDocument": {"uri": path_to_file_uri(path)?, "version": version},
                "contentChanges": [{"text": text}]
            }),
        )
    }

    pub fn did_close(&self, path: &Path) -> Result<()> {
        self.notify(
            "textDocument/didClose",
            json!({"textDocument": {"uri": path_to_file_uri(path)?}}),
        )
    }

    pub fn definition(&mut self, path: &Path, line: usize, character: usize) -> Result<u64> {
        self.request(
            "textDocument/definition",
            json!({
                "textDocument": {"uri": path_to_file_uri(path)?},
                "position": {"line": line, "character": character}
            }),
        )
    }

    pub fn respond(&self, id: u64, result: Value) -> Result<()> {
        self.write(json!({"jsonrpc": "2.0", "id": id, "result": result}))
    }

    pub fn drain_events(&self) -> impl Iterator<Item = LspEvent> + '_ {
        self.events.try_iter()
    }

    fn request(&mut self, method: &str, params: Value) -> Result<u64> {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        self.write(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))?;
        Ok(id)
    }

    fn notify(&self, method: &str, params: Value) -> Result<()> {
        self.write(json!({"jsonrpc": "2.0", "method": method, "params": params}))
    }

    fn write(&self, message: Value) -> Result<()> {
        let body = serde_json::to_vec(&message)?;
        let mut stdin = self
            .stdin
            .lock()
            .map_err(|_| anyhow::anyhow!("LSP stdin poisoned"))?;
        write!(stdin, "Content-Length: {}\r\n\r\n", body.len())?;
        stdin.write_all(&body)?;
        stdin.flush()?;
        Ok(())
    }
}

impl Drop for LspClient {
    fn drop(&mut self) {
        let _ = self.notify("exit", json!({}));
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn read_server_messages(stdout: impl std::io::Read, sender: mpsc::Sender<LspEvent>) {
    let mut reader = BufReader::new(stdout);
    loop {
        match read_message(&mut reader) {
            Ok(Some(message)) => {
                if let Some(event) = parse_event(message) {
                    let _ = sender.send(event);
                }
            }
            Ok(None) => break,
            Err(error) => {
                let _ = sender.send(LspEvent::Failed(error.to_string()));
                break;
            }
        }
    }
}

fn read_message(reader: &mut impl BufRead) -> Result<Option<Value>> {
    let mut content_length = None;
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header)? == 0 {
            return Ok(None);
        }
        if header == "\r\n" || header == "\n" {
            break;
        }
        if let Some(value) = header
            .split_once(':')
            .filter(|(name, _)| name.eq_ignore_ascii_case("content-length"))
            .map(|(_, value)| value.trim())
        {
            content_length = Some(value.parse::<usize>()?);
        }
    }
    let length = content_length.context("LSP message is missing Content-Length")?;
    let mut body = vec![0; length];
    reader.read_exact(&mut body)?;
    Ok(Some(serde_json::from_slice(&body)?))
}

fn parse_event(message: Value) -> Option<LspEvent> {
    if message.get("method").and_then(Value::as_str) == Some("textDocument/publishDiagnostics") {
        let params = message.get("params")?;
        let path = uri_to_path(params.get("uri")?.as_str()?)?;
        let items = params
            .get("diagnostics")?
            .as_array()?
            .iter()
            .filter_map(|item| parse_diagnostic(&path, item))
            .collect();
        return Some(LspEvent::Diagnostics { path, items });
    }
    let id = message.get("id")?.as_u64()?;
    if let Some(method) = message.get("method").and_then(Value::as_str) {
        return Some(LspEvent::ServerRequest {
            id,
            method: method.to_owned(),
            params: message.get("params").cloned().unwrap_or(Value::Null),
        });
    }
    Some(LspEvent::Response {
        id,
        result: message.get("result").cloned().unwrap_or(Value::Null),
    })
}

fn parse_diagnostic(path: &Path, value: &Value) -> Option<LspDiagnostic> {
    let range = value.get("range")?;
    let start = range.get("start")?;
    let end = range.get("end")?;
    Some(LspDiagnostic {
        path: path.to_path_buf(),
        start_line: start.get("line")?.as_u64()? as usize,
        start_character: start.get("character")?.as_u64()? as usize,
        end_line: end.get("line")?.as_u64()? as usize,
        end_character: end.get("character")?.as_u64()? as usize,
        severity: value.get("severity").and_then(Value::as_u64).unwrap_or(3),
        message: value.get("message")?.as_str()?.to_owned(),
    })
}

pub fn parse_definition_locations(result: &Value) -> Vec<DefinitionLocation> {
    let values = match result {
        Value::Array(values) => values.iter().collect::<Vec<_>>(),
        Value::Object(_) => vec![result],
        _ => Vec::new(),
    };
    values
        .into_iter()
        .filter_map(|value| {
            let uri = value
                .get("targetUri")
                .or_else(|| value.get("uri"))?
                .as_str()?;
            let range = value
                .get("targetSelectionRange")
                .or_else(|| value.get("targetRange"))
                .or_else(|| value.get("range"))?;
            let start = range.get("start")?;
            let end = range.get("end")?;
            Some(DefinitionLocation {
                path: uri_to_path(uri)?,
                start_line: start.get("line")?.as_u64()? as usize,
                start_character: start.get("character")?.as_u64()? as usize,
                end_line: end.get("line")?.as_u64()? as usize,
                end_character: end.get("character")?.as_u64()? as usize,
            })
        })
        .collect()
}

fn path_to_file_uri(path: &Path) -> Result<String> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    Url::from_file_path(&absolute)
        .map(String::from)
        .map_err(|()| anyhow::anyhow!("could not convert {} to a file URI", absolute.display()))
}

fn uri_to_path(uri: &str) -> Option<PathBuf> {
    Url::parse(uri).ok()?.to_file_path().ok()
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;

    #[test]
    fn protocol_reader_respects_content_length() {
        let body = br#"{"jsonrpc":"2.0","id":7,"result":null}"#;
        let mut message = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
        message.extend_from_slice(body);
        let parsed = read_message(&mut Cursor::new(message))
            .expect("message")
            .unwrap();
        assert_eq!(parsed["id"], 7);
    }

    #[test]
    fn file_uris_and_definition_links_round_trip_spaces() {
        let path = Path::new("/tmp/Textify Project/main.rs");
        let uri = path_to_file_uri(path).expect("file URI");
        assert_eq!(uri_to_path(&uri).as_deref(), Some(path));
        let result = json!([{
            "targetUri": uri,
            "targetSelectionRange": {
                "start": {"line": 4, "character": 2},
                "end": {"line": 4, "character": 8}
            }
        }]);
        let definitions = parse_definition_locations(&result);
        assert_eq!(definitions[0].path, path);
        assert_eq!(definitions[0].start_line, 4);
    }

    #[cfg(windows)]
    #[test]
    fn windows_drive_paths_round_trip_through_file_uris() {
        let path = Path::new(r"C:\Users\Textify Project\main.rs");
        let uri = path_to_file_uri(path).expect("file URI");
        assert_eq!(uri_to_path(&uri).as_deref(), Some(path));
    }

    #[test]
    fn diagnostics_notification_is_parsed() {
        let event = parse_event(json!({
            "jsonrpc": "2.0",
            "method": "textDocument/publishDiagnostics",
            "params": {
                "uri": "file:///tmp/main.rs",
                "diagnostics": [{
                    "range": {
                        "start": {"line": 1, "character": 2},
                        "end": {"line": 1, "character": 5}
                    },
                    "severity": 1,
                    "message": "broken"
                }]
            }
        }))
        .expect("diagnostics");
        let LspEvent::Diagnostics { items, .. } = event else {
            panic!("unexpected event")
        };
        assert_eq!(items[0].message, "broken");
        assert_eq!(items[0].severity, 1);
    }

    #[test]
    fn server_requests_are_not_mistaken_for_responses() {
        let event = parse_event(json!({
            "jsonrpc": "2.0",
            "id": 9,
            "method": "workspace/configuration",
            "params": {"items": [{"section": "rust-analyzer"}]}
        }))
        .expect("request");
        let LspEvent::ServerRequest { id, method, .. } = event else {
            panic!("unexpected event")
        };
        assert_eq!(id, 9);
        assert_eq!(method, "workspace/configuration");
    }
}
