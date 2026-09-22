#![forbid(unsafe_code)]

use std::net::{TcpStream, ToSocketAddrs};
use std::sync::Mutex;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use nova_client::Client;
use nova_core::error::NovaError;
use nova_executor::MemoryBackend;
use nova_server::{NovaServer, ServerConfig, ShutdownHandle};
use serde::{Deserialize, Serialize};
use tauri::State;

const MAX_QUERY_BYTES: usize = 1024 * 1024;
const MAX_TIMEOUT_MILLIS: u64 = 60_000;
const MAX_RESPONSE_BYTES: usize = 16 * 1024 * 1024;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProbeRequest {
    address: String,
    timeout_millis: u64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExecuteRequest {
    address: String,
    token: Option<String>,
    query: String,
    timeout_millis: u64,
    maximum_response_bytes: usize,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ConnectionResponse {
    address: String,
    mode: &'static str,
    message: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct QueryResponse {
    request_id: u64,
    payload: String,
    elapsed_millis: u64,
}

#[derive(Debug, Serialize)]
struct CommandError {
    kind: &'static str,
    message: String,
}

impl From<NovaError> for CommandError {
    fn from(error: NovaError) -> Self {
        let kind = match error {
            NovaError::InvalidArgument(_) => "InvalidArgument",
            NovaError::NotFound(_) => "NotFound",
            NovaError::AlreadyExists(_) => "AlreadyExists",
            NovaError::Unsupported(_) => "Unsupported",
            NovaError::Storage(_) => "Storage",
            NovaError::Corruption(_) => "Protocol",
            NovaError::Auth(_) => "Authentication",
            NovaError::PermissionDenied(_) => "PermissionDenied",
            NovaError::InvalidState(_) => "InvalidState",
            NovaError::Timeout => "Timeout",
            NovaError::Io(_) => "Network",
            NovaError::Internal(_) => "Internal",
        };
        Self {
            kind,
            message: error.to_string(),
        }
    }
}

impl CommandError {
    fn invalid(message: impl Into<String>) -> Self {
        Self {
            kind: "InvalidArgument",
            message: message.into(),
        }
    }

    fn internal(message: impl Into<String>) -> Self {
        Self {
            kind: "Internal",
            message: message.into(),
        }
    }
}

struct DemoServer {
    address: String,
    shutdown: ShutdownHandle,
    worker: Option<JoinHandle<nova_core::error::Result<()>>>,
}

impl Drop for DemoServer {
    fn drop(&mut self) {
        self.shutdown.shutdown();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[derive(Default)]
struct StudioState {
    demo: Mutex<Option<DemoServer>>,
}

#[tauri::command]
async fn probe_connection(request: ProbeRequest) -> Result<ConnectionResponse, CommandError> {
    tauri::async_runtime::spawn_blocking(move || probe_connection_inner(&request))
        .await
        .map_err(|error| CommandError::internal(format!("connection task failed: {error}")))?
}

fn probe_connection_inner(request: &ProbeRequest) -> Result<ConnectionResponse, CommandError> {
    validate_timeout(request.timeout_millis)?;
    let address = resolve_address(&request.address)?;
    TcpStream::connect_timeout(&address, Duration::from_millis(request.timeout_millis))
        .map_err(NovaError::from)?;
    Ok(ConnectionResponse {
        address: address.to_string(),
        mode: "remote",
        message: "TCP connection succeeded".to_owned(),
    })
}

#[tauri::command]
async fn execute_query(request: ExecuteRequest) -> Result<QueryResponse, CommandError> {
    tauri::async_runtime::spawn_blocking(move || execute_query_inner(request))
        .await
        .map_err(|error| CommandError::internal(format!("query task failed: {error}")))?
}

fn execute_query_inner(request: ExecuteRequest) -> Result<QueryResponse, CommandError> {
    validate_timeout(request.timeout_millis)?;
    if request.query.trim().is_empty() {
        return Err(CommandError::invalid("NovaQL query cannot be empty"));
    }
    if request.query.len() > MAX_QUERY_BYTES {
        return Err(CommandError::invalid(format!(
            "NovaQL query exceeds {MAX_QUERY_BYTES} bytes"
        )));
    }
    if request.maximum_response_bytes == 0 || request.maximum_response_bytes > MAX_RESPONSE_BYTES {
        return Err(CommandError::invalid(format!(
            "response limit must be between 1 and {MAX_RESPONSE_BYTES} bytes"
        )));
    }
    let mut client = Client::connect(request.address)?;
    client.set_limits(
        Duration::from_millis(request.timeout_millis),
        request.maximum_response_bytes,
    )?;
    if let Some(token) = request.token.filter(|token| !token.is_empty()) {
        client.set_token(token);
    }
    let started = Instant::now();
    let response = client.execute(request.query)?;
    Ok(QueryResponse {
        request_id: response.request_id,
        payload: response.payload,
        elapsed_millis: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
    })
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
fn start_demo_server(state: State<'_, StudioState>) -> Result<ConnectionResponse, CommandError> {
    let mut demo = state
        .demo
        .lock()
        .map_err(|_| CommandError::internal("demo server state mutex poisoned"))?;
    if let Some(server) = demo.as_ref() {
        return Ok(demo_response(&server.address));
    }
    let server = NovaServer::bind("127.0.0.1:0", MemoryBackend::new(), ServerConfig::default())?;
    let address = server.local_addr()?.to_string();
    let shutdown = server.shutdown_handle();
    let worker = thread::spawn(move || server.serve());
    *demo = Some(DemoServer {
        address: address.clone(),
        shutdown,
        worker: Some(worker),
    });
    Ok(demo_response(&address))
}

fn demo_response(address: &str) -> ConnectionResponse {
    ConnectionResponse {
        address: address.to_owned(),
        mode: "demo",
        message: "Session-only demo server is running; data is not persisted".to_owned(),
    }
}

fn validate_timeout(timeout_millis: u64) -> Result<(), CommandError> {
    if timeout_millis == 0 || timeout_millis > MAX_TIMEOUT_MILLIS {
        return Err(CommandError::invalid(format!(
            "timeout must be between 1 and {MAX_TIMEOUT_MILLIS} milliseconds"
        )));
    }
    Ok(())
}

fn resolve_address(address: &str) -> Result<std::net::SocketAddr, CommandError> {
    address
        .to_socket_addrs()
        .map_err(NovaError::from)?
        .next()
        .ok_or_else(|| CommandError::invalid("server address resolved empty"))
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
/// Starts the native Studio application.
///
/// # Panics
/// Panics if Tauri cannot initialize the configured native runtime or window.
pub fn run() {
    tauri::Builder::default()
        .manage(StudioState::default())
        .invoke_handler(tauri::generate_handler![
            probe_connection,
            execute_query,
            start_demo_server
        ])
        .run(tauri::generate_context!())
        .expect("failed to run NovaDB Studio");
}

#[cfg(test)]
mod tests {
    use nova_executor::ExecutionBackend;

    use super::*;

    #[test]
    fn validation_rejects_unbounded_requests() {
        assert!(validate_timeout(0).is_err());
        assert!(validate_timeout(MAX_TIMEOUT_MILLIS + 1).is_err());
        let request = ExecuteRequest {
            address: "127.0.0.1:1".to_owned(),
            token: None,
            query: String::new(),
            timeout_millis: 1,
            maximum_response_bytes: 1,
        };
        assert!(execute_query_inner(request).is_err());
    }

    #[test]
    fn native_client_executes_against_a_real_local_server() {
        let mut backend = MemoryBackend::new();
        backend.create_collection("students").unwrap();
        let server = NovaServer::bind("127.0.0.1:0", backend, ServerConfig::default()).unwrap();
        let address = server.local_addr().unwrap();
        let shutdown = server.shutdown_handle();
        let worker = thread::spawn(move || server.serve());
        let response = execute_query_inner(ExecuteRequest {
            address: address.to_string(),
            token: None,
            query: "students.get {}".to_owned(),
            timeout_millis: 1_000,
            maximum_response_bytes: 1024 * 1024,
        })
        .unwrap();
        assert!(response.payload.starts_with("Documents("));
        shutdown.shutdown();
        worker.join().unwrap().unwrap();
    }
}
