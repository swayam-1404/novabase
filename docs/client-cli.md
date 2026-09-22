# NovaDB Client SDK and CLI (Phase 16)

`nova-client` is a synchronous Rust client for server protocol v2. A client
resolves its server address once, opens one bounded connection per query,
assigns monotonically increasing request ids, applies read/write/connect
timeouts, verifies response correlation, and caps response payload size.

Bearer tokens are optional for explicitly anonymous servers and required by
authenticated servers. Debug output always redacts configured tokens. Wire
errors map back to structured `NovaError` variants: authentication,
authorization, corruption, invalid query/execution, busy state, or internal
failure.

The `nova` binary exposes:

```text
nova [--address HOST:PORT] [--token TOKEN] query <NovaQL>
nova [--address HOST:PORT] [--token TOKEN] shell
nova version
```

The shell reads one NovaQL command per line and continues after query errors;
`exit`, `quit`, or end-of-input closes it. `NOVADB_ADDRESS` defaults to
`127.0.0.1:7400`, and `NOVADB_TOKEN` supplies a session without placing it in
the command line. Explicit options override environment values.

This phase supplies a client, not a daemon launcher. Embedding applications
construct `NovaServer` with their chosen backend and authentication bootstrap;
packaging a standalone configured daemon remains deployment work.
