//! Synchronous NovaDB SDK client for the versioned TCP protocol.

#![forbid(unsafe_code)]

use std::fmt;
use std::io::Write;
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::time::Duration;

use nova_core::error::{NovaError, Result};
use nova_server::{encode_request, read_response, ErrorCode, Request};

const DEFAULT_MAX_RESPONSE: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientResponse {
    pub request_id: u64,
    pub payload: String,
}

pub struct Client {
    address: SocketAddr,
    token: Option<String>,
    timeout: Duration,
    maximum_response_bytes: usize,
    next_request_id: u64,
}

impl fmt::Debug for Client {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Client")
            .field("address", &self.address)
            .field("token", &self.token.as_ref().map(|_| "[REDACTED]"))
            .field("timeout", &self.timeout)
            .field("maximum_response_bytes", &self.maximum_response_bytes)
            .field("next_request_id", &self.next_request_id)
            .finish()
    }
}

impl Client {
    /// Resolves a server address using safe default limits.
    ///
    /// # Errors
    /// Returns a typed resolution error or `InvalidArgument` for no address.
    pub fn connect(address: impl ToSocketAddrs) -> Result<Self> {
        let address = address.to_socket_addrs()?.next().ok_or_else(|| {
            NovaError::InvalidArgument("server address resolved empty".to_owned())
        })?;
        Ok(Self {
            address,
            token: None,
            timeout: Duration::from_secs(5),
            maximum_response_bytes: DEFAULT_MAX_RESPONSE,
            next_request_id: 1,
        })
    }

    pub fn set_token(&mut self, token: impl Into<String>) {
        self.token = Some(token.into());
    }

    pub fn clear_token(&mut self) {
        self.token = None;
    }

    /// Sets positive socket and response-size limits.
    ///
    /// # Errors
    /// Returns `InvalidArgument` for a zero timeout or response limit.
    pub fn set_limits(&mut self, timeout: Duration, maximum_response_bytes: usize) -> Result<()> {
        if timeout.is_zero() || maximum_response_bytes == 0 {
            return Err(NovaError::InvalidArgument(
                "client timeout and response limit must be positive".to_owned(),
            ));
        }
        self.timeout = timeout;
        self.maximum_response_bytes = maximum_response_bytes;
        Ok(())
    }

    /// Sends one `NovaQL` query over a fresh connection.
    ///
    /// # Errors
    /// Returns typed network, protocol, auth, busy, parse, or execution errors.
    pub fn execute(&mut self, query: impl Into<String>) -> Result<ClientResponse> {
        let request_id = self.next_request_id;
        self.next_request_id = self
            .next_request_id
            .checked_add(1)
            .ok_or_else(|| NovaError::InvalidState("client request id overflow".to_owned()))?;
        let request = Request {
            request_id,
            auth_token: self.token.clone(),
            query: query.into(),
        };
        let mut stream = TcpStream::connect_timeout(&self.address, self.timeout)?;
        stream.set_read_timeout(Some(self.timeout))?;
        stream.set_write_timeout(Some(self.timeout))?;
        stream.write_all(&encode_request(&request)?)?;
        let response = read_response(&mut stream, self.maximum_response_bytes)?;
        if response.request_id != request_id {
            return Err(NovaError::Corruption(format!(
                "response id {} does not match request id {request_id}",
                response.request_id
            )));
        }
        match response.outcome {
            Ok(payload) => Ok(ClientResponse {
                request_id,
                payload,
            }),
            Err((code, message)) => Err(map_server_error(code, message)),
        }
    }
}

fn map_server_error(code: ErrorCode, message: String) -> NovaError {
    match code {
        ErrorCode::Authentication => NovaError::Auth(message),
        ErrorCode::Authorization => NovaError::PermissionDenied(message),
        ErrorCode::Protocol => NovaError::Corruption(message),
        ErrorCode::Parse | ErrorCode::Execution => NovaError::InvalidArgument(message),
        ErrorCode::Busy => NovaError::InvalidState(message),
        ErrorCode::Internal => NovaError::Internal(message),
    }
}

#[cfg(test)]
mod tests {
    use std::thread;

    use nova_executor::{ExecutionBackend, MemoryBackend};
    use nova_server::{NovaServer, ServerConfig};

    use super::*;

    #[test]
    fn client_executes_correlated_requests_and_maps_errors() {
        let mut backend = MemoryBackend::new();
        backend.create_collection("students").unwrap();
        let server = NovaServer::bind("127.0.0.1:0", backend, ServerConfig::default()).unwrap();
        let address = server.local_addr().unwrap();
        let shutdown = server.shutdown_handle();
        let server_thread = thread::spawn(move || server.serve());
        let mut client = Client::connect(address).unwrap();
        let first = client.execute("students.get {}").unwrap();
        let second = client.execute("students.get {}").unwrap();
        assert_eq!((first.request_id, second.request_id), (1, 2));
        assert!(first.payload.starts_with("Documents("));
        assert!(matches!(
            client.execute("invalid query"),
            Err(NovaError::InvalidArgument(_))
        ));
        shutdown.shutdown();
        server_thread.join().unwrap().unwrap();
    }

    #[test]
    fn token_is_redacted_and_limits_are_validated() {
        let mut client = Client::connect("127.0.0.1:7400").unwrap();
        client.set_token("secret-token");
        let debug = format!("{client:?}");
        assert!(debug.contains("[REDACTED]"));
        assert!(!debug.contains("secret-token"));
        assert!(client.set_limits(Duration::ZERO, 1).is_err());
        assert!(client.set_limits(Duration::from_secs(1), 0).is_err());
    }
}
