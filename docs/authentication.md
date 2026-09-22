# NovaDB Authentication and Authorization (Phase 15)

`nova-auth` separates credential verification from deterministic authorization.
Passwords are never retained: each is hashed with Argon2id and a fresh
16-byte operating-system-random salt using the audited `argon2` crate.
Verification uses that library's password verifier. Unknown users are checked
against a dummy Argon2 hash and return the same `invalid credentials` error as
wrong passwords or disabled accounts.

Successful authentication creates a 256-bit operating-system-random bearer
token. Token debug output is always redacted. Sessions are process-local and
can be explicitly revoked. Password changes, role changes, and disabling a
user revoke all sessions owned by that user.

## Roles

| Role | Read | Write | Manage collection/index DDL |
|------|------|-------|-----------------------------|
| ReadOnly | yes | no | no |
| ReadWrite | yes | yes | no |
| Admin | yes | yes | yes |

An authenticated server maps parsed commands to one of those permissions and
authorizes before backend access. Missing/invalid sessions produce a distinct
authentication response; insufficient roles produce authorization errors.

User creation and role administration are embedding/bootstrap APIs in this
phase and must be exposed only through an administrator-controlled environment.
The prototype does not claim TLS, external identity federation, password reset,
multi-factor authentication, or durable session persistence. Network
deployments must place the server behind a confidential transport because
bearer tokens grant their holder the session's authority.
