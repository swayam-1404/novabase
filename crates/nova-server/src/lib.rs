//! Bounded single-node TCP server for the NovaDB wire protocol.

#![forbid(unsafe_code)]

mod protocol;

use std::io;
use std::net::{SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use nova_auth::{AuthService, Permission};
use nova_core::error::{NovaError, Result};
use nova_executor::{execute, ExecutionBackend};
use nova_query::Command;

pub use protocol::{
    encode_request, read_request, read_response, write_response, ErrorCode, Request, Response,
};

/// Bounded server resource and timeout settings.
#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub max_connections: usize,
    pub max_payload_bytes: usize,
    pub connection_timeout: Duration,
    pub poll_interval: Duration,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            max_connections: 64,
            max_payload_bytes: 1024 * 1024,
            connection_timeout: Duration::from_secs(5),
            poll_interval: Duration::from_millis(10),
        }
    }
}

/// Cloneable graceful-shutdown signal.
#[derive(Debug, Clone)]
pub struct ShutdownHandle(Arc<AtomicBool>);

impl ShutdownHandle {
    pub fn shutdown(&self) {
        self.0.store(true, Ordering::Release);
    }
}

/// A bound server that owns its backend and listener.
pub struct NovaServer<B> {
    listener: TcpListener,
    backend: Arc<Mutex<B>>,
    config: ServerConfig,
    shutdown: ShutdownHandle,
    auth: Option<Arc<AuthService>>,
}

impl<B: ExecutionBackend + Send + 'static> NovaServer<B> {
    /// Binds a server after validating bounded-resource configuration.
    ///
    /// # Errors
    /// Returns a typed argument, address-resolution, or bind error.
    pub fn bind(address: impl ToSocketAddrs, backend: B, config: ServerConfig) -> Result<Self> {
        Self::bind_with_auth(address, backend, config, None)
    }

    /// Binds a server that rejects requests without an authorized bearer token.
    ///
    /// # Errors
    /// Returns a typed argument, address-resolution, or bind error.
    pub fn bind_authenticated(
        address: impl ToSocketAddrs,
        backend: B,
        config: ServerConfig,
        auth: Arc<AuthService>,
    ) -> Result<Self> {
        Self::bind_with_auth(address, backend, config, Some(auth))
    }

    fn bind_with_auth(
        address: impl ToSocketAddrs,
        backend: B,
        config: ServerConfig,
        auth: Option<Arc<AuthService>>,
    ) -> Result<Self> {
        if config.max_connections == 0 || config.max_payload_bytes == 0 {
            return Err(NovaError::InvalidArgument(
                "server connection and payload limits must be positive".to_owned(),
            ));
        }
        let listener = TcpListener::bind(address)?;
        listener.set_nonblocking(true)?;
        Ok(Self {
            listener,
            backend: Arc::new(Mutex::new(backend)),
            config,
            shutdown: ShutdownHandle(Arc::new(AtomicBool::new(false))),
            auth,
        })
    }

    /// Returns the actual bound address.
    ///
    /// # Errors
    /// Returns a typed socket error if the address cannot be read.
    pub fn local_addr(&self) -> Result<SocketAddr> {
        Ok(self.listener.local_addr()?)
    }

    #[must_use]
    pub fn shutdown_handle(&self) -> ShutdownHandle {
        self.shutdown.clone()
    }

    /// Serves connections until shutdown, then joins every worker.
    ///
    /// # Errors
    /// Returns a typed listener or worker-panic error.
    pub fn serve(self) -> Result<()> {
        let active = Arc::new(AtomicUsize::new(0));
        let mut workers: Vec<JoinHandle<Result<()>>> = Vec::new();
        while !self.shutdown.0.load(Ordering::Acquire) {
            match self.listener.accept() {
                Ok((stream, _)) => {
                    if active.load(Ordering::Acquire) >= self.config.max_connections {
                        reject_busy(stream, &self.config)?;
                        continue;
                    }
                    active.fetch_add(1, Ordering::AcqRel);
                    let backend = Arc::clone(&self.backend);
                    let active_count = Arc::clone(&active);
                    let config = self.config.clone();
                    let auth = self.auth.clone();
                    workers.push(thread::spawn(move || {
                        let result = handle_connection(stream, &backend, &config, auth.as_deref());
                        active_count.fetch_sub(1, Ordering::AcqRel);
                        result
                    }));
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    thread::sleep(self.config.poll_interval);
                }
                Err(error) => return Err(error.into()),
            }
        }
        for worker in workers {
            worker
                .join()
                .map_err(|_| NovaError::Internal("server worker panicked".to_owned()))??;
        }
        Ok(())
    }
}

fn configure_stream(stream: &TcpStream, config: &ServerConfig) -> Result<()> {
    stream.set_read_timeout(Some(config.connection_timeout))?;
    stream.set_write_timeout(Some(config.connection_timeout))?;
    Ok(())
}

fn handle_connection<B: ExecutionBackend>(
    mut stream: TcpStream,
    backend: &Mutex<B>,
    config: &ServerConfig,
    auth: Option<&AuthService>,
) -> Result<()> {
    configure_stream(&stream, config)?;
    let request = match read_request(&mut stream, config.max_payload_bytes) {
        Ok(request) => request,
        Err(error) => {
            return write_response(
                &mut stream,
                &Response {
                    request_id: 0,
                    outcome: Err((ErrorCode::Protocol, error.to_string())),
                },
            );
        }
    };
    let outcome = match nova_query::parse(&request.query) {
        Ok(query) => match authorize(auth, request.auth_token.as_deref(), &query.command) {
            Ok(()) => match backend.lock() {
                Ok(mut backend) => execute(&query, &mut *backend)
                    .map(|result| format!("{result:?}"))
                    .map_err(|error| (ErrorCode::Execution, error.to_string())),
                Err(_) => Err((
                    ErrorCode::Internal,
                    "server backend mutex poisoned".to_owned(),
                )),
            },
            Err(error) => Err(error),
        },
        Err(error) => Err((ErrorCode::Parse, error.to_string())),
    };
    write_response(
        &mut stream,
        &Response {
            request_id: request.request_id,
            outcome,
        },
    )
}

fn authorize(
    auth: Option<&AuthService>,
    token: Option<&str>,
    command: &Command,
) -> std::result::Result<(), (ErrorCode, String)> {
    let Some(auth) = auth else {
        return Ok(());
    };
    let token = token.ok_or_else(|| {
        (
            ErrorCode::Authentication,
            "authentication token required".to_owned(),
        )
    })?;
    let permission = match command {
        Command::Get { .. } => Permission::Read,
        Command::Insert { .. } | Command::Update { .. } | Command::Delete { .. } => {
            Permission::Write
        }
        Command::CreateCollection { .. }
        | Command::DropCollection { .. }
        | Command::CreateIndex { .. }
        | Command::DropIndex { .. } => Permission::Manage,
    };
    auth.authorize(token, permission)
        .map(|_| ())
        .map_err(|error| match error {
            NovaError::PermissionDenied(_) => (ErrorCode::Authorization, error.to_string()),
            NovaError::Auth(_) => (ErrorCode::Authentication, error.to_string()),
            other => (ErrorCode::Internal, other.to_string()),
        })
}

fn reject_busy(mut stream: TcpStream, config: &ServerConfig) -> Result<()> {
    configure_stream(&stream, config)?;
    let request_id =
        read_request(&mut stream, config.max_payload_bytes).map_or(0, |request| request.request_id);
    write_response(
        &mut stream,
        &Response {
            request_id,
            outcome: Err((
                ErrorCode::Busy,
                "server connection limit reached".to_owned(),
            )),
        },
    )
}

#[cfg(test)]
mod tests {
    use std::io::{Cursor, Write};

    use nova_executor::{ExecutionBackend, MemoryBackend};

    use super::*;

    #[test]
    fn protocol_round_trips_and_rejects_corruption_and_limits() {
        let request = Request {
            request_id: 42,
            auth_token: Some("token".to_owned()),
            query: "students.get {}".to_owned(),
        };
        let encoded = encode_request(&request).unwrap();
        assert_eq!(
            read_request(&mut Cursor::new(&encoded), 100).unwrap(),
            request
        );
        assert!(read_request(&mut Cursor::new(&encoded), 2).is_err());
        let mut corrupt = encoded;
        let last = corrupt.len() - 1;
        corrupt[last] ^= 1;
        assert!(read_request(&mut Cursor::new(corrupt), 100).is_err());
    }

    #[test]
    fn tcp_server_executes_queries_reports_parse_errors_and_shuts_down() {
        let mut backend = MemoryBackend::new();
        backend.create_collection("students").unwrap();
        let server = NovaServer::bind("127.0.0.1:0", backend, ServerConfig::default()).unwrap();
        let address = server.local_addr().unwrap();
        let shutdown = server.shutdown_handle();
        let thread = thread::spawn(move || server.serve());

        let response = send(address, 7, None, "students.get {}");
        assert_eq!(response.request_id, 7);
        assert!(response.outcome.unwrap().starts_with("Documents("));

        let response = send(address, 8, None, "not valid NovaQL");
        assert_eq!(response.request_id, 8);
        assert!(matches!(response.outcome, Err((ErrorCode::Parse, _))));
        shutdown.shutdown();
        thread.join().unwrap().unwrap();
    }

    #[test]
    fn authenticated_server_enforces_roles_before_execution() {
        let auth = Arc::new(AuthService::new().unwrap());
        auth.create_user("reader", "reader-password", nova_auth::Role::ReadOnly)
            .unwrap();
        let token = auth
            .authenticate("reader", "reader-password")
            .unwrap()
            .into_string();
        let mut backend = MemoryBackend::new();
        backend.create_collection("students").unwrap();
        let server =
            NovaServer::bind_authenticated("127.0.0.1:0", backend, ServerConfig::default(), auth)
                .unwrap();
        let address = server.local_addr().unwrap();
        let shutdown = server.shutdown_handle();
        let thread = thread::spawn(move || server.serve());
        assert!(matches!(
            send(address, 1, None, "students.get {}").outcome,
            Err((ErrorCode::Authentication, _))
        ));
        assert!(send(address, 2, Some(&token), "students.get {}")
            .outcome
            .is_ok());
        assert!(matches!(
            send(address, 3, Some(&token), "students.insert {name: \"Ada\"}").outcome,
            Err((ErrorCode::Authorization, _))
        ));
        shutdown.shutdown();
        thread.join().unwrap().unwrap();
    }

    fn send(
        address: SocketAddr,
        request_id: u64,
        auth_token: Option<&str>,
        query: &str,
    ) -> Response {
        let mut stream = TcpStream::connect(address).unwrap();
        stream
            .write_all(
                &encode_request(&Request {
                    request_id,
                    auth_token: auth_token.map(str::to_owned),
                    query: query.to_owned(),
                })
                .unwrap(),
            )
            .unwrap();
        read_response(&mut stream, 1024 * 1024).unwrap()
    }
}
