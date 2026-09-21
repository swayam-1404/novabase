# NovaDB Transactions and Locking (Phase 13)

`nova-transaction` provides a thread-safe transaction state machine and strict
two-phase locking at collection granularity.

Transactions move exactly once from `Active` to `Committed` or `RolledBack`.
Begin, commit, and rollback append corresponding WAL records. Commit and
rollback flush their terminal record before releasing locks, so another
transaction cannot observe a released lock before the owner's outcome is
durable.

Shared locks coexist with other shared locks. An exclusive lock requires no
foreign reader or writer. A transaction may re-enter its locks and may upgrade
from shared to exclusive when it is the only reader. Locks are retained until
the terminal transition (strict 2PL).

Conflict handling is deliberately fail-fast: acquisition returns
`NovaError::InvalidState` instead of waiting. This avoids hidden deadlock and
timeout behavior; callers may retry at a higher layer. Unknown, committed, and
rolled-back transaction ids cannot acquire locks or transition again.

The current coordinator establishes isolation ownership and durable outcomes.
It does not claim MVCC, row/document locks, distributed transactions, or
automatic undo of arbitrary external side effects. Data/index operations must
be performed while holding the appropriate lock and must log recoverable page
changes before page flush.
