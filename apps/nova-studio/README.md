# NovaDB Studio

NovaDB Studio is a native Tauri desktop workspace for the project’s real TCP
protocol and NovaQL language. It provides connection/token handling, a NovaQL
editor, Explain, templates, raw protocol results, saved collection shortcuts,
session history, and a session-only embedded demo server.

The current NovaDB protocol returns debug-text query results and does not expose
collection discovery. Studio therefore does not fabricate a document grid or
catalog. Results are displayed exactly as returned, and saved collection names
are local navigation shortcuts. A typed-result/catalog protocol can enable
Compass-style grids in a later protocol version.

## Development

```powershell
cd apps\nova-studio
npm install
npm run tauri dev
```

Use **Demo** for an in-memory session, or connect to a running `NovaServer` TCP
address. Demo data disappears when Studio exits. Bearer tokens and query history
are held in memory and are not written to local storage.

Build the optimized Windows executable with:

```powershell
npm run tauri build
```

With bundling disabled for this research prototype, the executable is written
to `src-tauri\target\release\nova-studio.exe`. It uses the Windows WebView2
runtime.

## Verification

```powershell
npm run build
cargo fmt --manifest-path src-tauri\Cargo.toml -- --check
cargo clippy --manifest-path src-tauri\Cargo.toml --all-targets -- -D warnings
cargo test --manifest-path src-tauri\Cargo.toml
```

The Tauri shell is excluded from the root Cargo workspace so Linux CI does not
require desktop WebKit development packages. The existing NovaDB workspace gate
remains unchanged.
