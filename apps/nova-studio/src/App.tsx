import { invoke } from "@tauri-apps/api/core";
import { useEffect, useMemo, useRef, useState } from "react";

type Theme = "dark" | "light";
type ConnectionMode = "remote" | "demo";

type QueryResponse = {
  requestId: number;
  payload: string;
  elapsedMillis: number;
};

type ConnectionResponse = {
  address: string;
  mode: ConnectionMode;
  message: string;
};

type NativeError = {
  kind?: string;
  message?: string;
};

type HistoryItem = {
  id: number;
  query: string;
  ok: boolean;
  elapsedMillis: number;
  timestamp: Date;
};

type Template = {
  label: string;
  description: string;
  query: string;
};

const templates: Template[] = [
  {
    label: "Read documents",
    description: "Filter nested document fields",
    query: 'students.get { address.state == "Odisha" } | limit 50',
  },
  {
    label: "Array membership",
    description: "Find values inside an array",
    query: 'students.get { skills contains "Rust" } | project name, skills',
  },
  {
    label: "Insert document",
    description: "Create a nested document",
    query:
      'students.insert { name: "Ada", branch: "CSE", skills: ["Rust", "Databases"], address: { state: "Odisha" } }',
  },
  {
    label: "Update documents",
    description: "Apply a document-native pipeline",
    query: 'students.update { branch == "CSE" } | set active = true',
  },
  {
    label: "Create collection",
    description: "Add a collection",
    query: "create collection students",
  },
  {
    label: "Create index",
    description: "Index a nested field path",
    query: "create index by_state on students (address.state)",
  },
];

const initialQuery = `students.get {
  branch == "CSE"
} | sort name asc | limit 25`;

function Icon({ name }: { name: string }) {
  const paths: Record<string, React.ReactNode> = {
    database: <><ellipse cx="12" cy="5" rx="7" ry="3"/><path d="M5 5v6c0 1.7 3.1 3 7 3s7-1.3 7-3V5"/><path d="M5 11v6c0 1.7 3.1 3 7 3s7-1.3 7-3v-6"/></>,
    play: <path d="m9 7 8 5-8 5V7Z"/>,
    bolt: <path d="m13 2-8 12h7l-1 8 8-12h-7l1-8Z"/>,
    copy: <><rect x="8" y="8" width="11" height="11" rx="2"/><path d="M16 8V5a2 2 0 0 0-2-2H5a2 2 0 0 0-2 2v9a2 2 0 0 0 2 2h3"/></>,
    plus: <><path d="M12 5v14M5 12h14"/></>,
    clock: <><circle cx="12" cy="12" r="9"/><path d="M12 7v5l3 2"/></>,
    terminal: <><path d="m4 7 4 4-4 4M10 17h10"/></>,
    sun: <><circle cx="12" cy="12" r="4"/><path d="M12 2v2M12 20v2M4.9 4.9l1.4 1.4M17.7 17.7l1.4 1.4M2 12h2M20 12h2M4.9 19.1l1.4-1.4M17.7 6.3l1.4-1.4"/></>,
    moon: <path d="M20 15.4A8 8 0 0 1 8.6 4 8 8 0 1 0 20 15.4Z"/>,
    chevron: <path d="m9 18 6-6-6-6"/>,
    trash: <><path d="M4 7h16M9 7V4h6v3M7 7l1 13h8l1-13"/></>,
    eye: <><path d="M2 12s3.5-6 10-6 10 6 10 6-3.5 6-10 6S2 12 2 12Z"/><circle cx="12" cy="12" r="2.5"/></>,
  };
  return <svg className="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">{paths[name]}</svg>;
}

function normalizeError(error: unknown): NativeError {
  if (typeof error === "string") {
    try {
      return JSON.parse(error) as NativeError;
    } catch {
      return { kind: "Error", message: error };
    }
  }
  if (error && typeof error === "object") return error as NativeError;
  return { kind: "Error", message: "Unknown native bridge error" };
}

function App() {
  const [theme, setTheme] = useState<Theme>(() => (localStorage.getItem("nova-theme") as Theme) || "dark");
  const [address, setAddress] = useState(() => localStorage.getItem("nova-address") || "127.0.0.1:7400");
  const [token, setToken] = useState("");
  const [showToken, setShowToken] = useState(false);
  const [connected, setConnected] = useState(false);
  const [mode, setMode] = useState<ConnectionMode>("remote");
  const [connectionMessage, setConnectionMessage] = useState("Not connected");
  const [connecting, setConnecting] = useState(false);
  const [query, setQuery] = useState(initialQuery);
  const [running, setRunning] = useState(false);
  const [result, setResult] = useState<QueryResponse | null>(null);
  const [error, setError] = useState<NativeError | null>(null);
  const [history, setHistory] = useState<HistoryItem[]>([]);
  const [collections, setCollections] = useState<string[]>(() => {
    try { return JSON.parse(localStorage.getItem("nova-collections") || "[]") as string[]; }
    catch { return []; }
  });
  const [newCollection, setNewCollection] = useState("");
  const [copied, setCopied] = useState(false);
  const editorRef = useRef<HTMLTextAreaElement>(null);

  const lineCount = useMemo(() => Math.max(1, query.split("\n").length), [query]);

  useEffect(() => {
    document.documentElement.dataset.theme = theme;
    localStorage.setItem("nova-theme", theme);
  }, [theme]);

  useEffect(() => localStorage.setItem("nova-address", address), [address]);
  useEffect(() => localStorage.setItem("nova-collections", JSON.stringify(collections)), [collections]);

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if ((event.ctrlKey || event.metaKey) && event.key === "Enter") {
        event.preventDefault();
        void runQuery();
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  });

  async function connectRemote() {
    setConnecting(true);
    setError(null);
    try {
      const response = await invoke<ConnectionResponse>("probe_connection", {
        request: { address, timeoutMillis: 3000 },
      });
      setConnected(true);
      setMode("remote");
      setConnectionMessage(response.message);
    } catch (nativeError) {
      setConnected(false);
      const parsed = normalizeError(nativeError);
      setError(parsed);
      setConnectionMessage(parsed.message || "Connection failed");
    } finally {
      setConnecting(false);
    }
  }

  async function startDemo() {
    setConnecting(true);
    setError(null);
    try {
      const response = await invoke<ConnectionResponse>("start_demo_server");
      setAddress(response.address);
      setToken("");
      setConnected(true);
      setMode("demo");
      setConnectionMessage(response.message);
    } catch (nativeError) {
      const parsed = normalizeError(nativeError);
      setError(parsed);
      setConnectionMessage(parsed.message || "Demo startup failed");
    } finally {
      setConnecting(false);
    }
  }

  async function runQuery(source = query) {
    const trimmed = source.trim();
    if (!trimmed || running) return;
    setRunning(true);
    setError(null);
    setResult(null);
    const started = performance.now();
    try {
      const response = await invoke<QueryResponse>("execute_query", {
        request: {
          address,
          token: token.trim() || null,
          query: trimmed,
          timeoutMillis: 5000,
          maximumResponseBytes: 16 * 1024 * 1024,
        },
      });
      setResult(response);
      setConnected(true);
      setHistory((items) => [{ id: Date.now(), query: trimmed, ok: true, elapsedMillis: response.elapsedMillis, timestamp: new Date() }, ...items].slice(0, 20));
    } catch (nativeError) {
      const parsed = normalizeError(nativeError);
      const elapsed = Math.max(0, Math.round(performance.now() - started));
      setError(parsed);
      setHistory((items) => [{ id: Date.now(), query: trimmed, ok: false, elapsedMillis: elapsed, timestamp: new Date() }, ...items].slice(0, 20));
    } finally {
      setRunning(false);
    }
  }

  function explainQuery() {
    const source = query.trim().replace(/^explain\s+/i, "");
    setQuery(`explain ${source}`);
    void runQuery(`explain ${source}`);
  }

  function addCollection() {
    const name = newCollection.trim();
    if (!name || !/^[\p{L}\p{N}_]+$/u.test(name) || collections.includes(name)) return;
    setCollections((current) => [...current, name].sort((a, b) => a.localeCompare(b)));
    setNewCollection("");
  }

  function chooseCollection(name: string) {
    setQuery(`${name}.get {} | limit 50`);
    editorRef.current?.focus();
  }

  async function copyResult() {
    if (!result?.payload) return;
    await navigator.clipboard.writeText(result.payload);
    setCopied(true);
    window.setTimeout(() => setCopied(false), 1200);
  }

  return (
    <div className="app-shell">
      <header className="topbar">
        <div className="brand">
          <div className="brand-mark"><span>N</span></div>
          <div><strong>NovaDB</strong><span>Studio</span></div>
          <span className="research-badge">research build</span>
        </div>
        <div className="topbar-actions">
          <div className={`connection-pill ${connected ? "online" : "offline"}`}>
            <span className="status-dot" />
            {connected ? `${mode === "demo" ? "Demo" : "Connected"} · ${address}` : "Disconnected"}
          </div>
          <button className="icon-button" onClick={() => setTheme(theme === "dark" ? "light" : "dark")} aria-label="Toggle theme">
            <Icon name={theme === "dark" ? "sun" : "moon"} />
          </button>
        </div>
      </header>

      <aside className="sidebar">
        <section className="sidebar-section connection-section">
          <div className="section-label">Connection</div>
          <label className="field-label" htmlFor="address">Server address</label>
          <input id="address" className="text-input" value={address} onChange={(event) => { setAddress(event.target.value); setConnected(false); setMode("remote"); }} spellCheck={false} />
          <label className="field-label" htmlFor="token">Bearer token <span>optional</span></label>
          <div className="token-field">
            <input id="token" className="text-input" type={showToken ? "text" : "password"} value={token} onChange={(event) => setToken(event.target.value)} placeholder="Not stored" autoComplete="off" />
            <button onClick={() => setShowToken((value) => !value)} aria-label="Show or hide token"><Icon name="eye" /></button>
          </div>
          <div className="connection-buttons">
            <button className="primary compact" onClick={() => void connectRemote()} disabled={connecting || !address.trim()}>{connecting ? "Checking…" : "Connect"}</button>
            <button className="secondary compact" onClick={() => void startDemo()} disabled={connecting}><Icon name="bolt" /> Demo</button>
          </div>
          <p className="connection-note">{connectionMessage}</p>
        </section>

        <section className="sidebar-section collections-section">
          <div className="section-row"><div className="section-label">Workspace collections</div><span>{collections.length}</span></div>
          <div className="add-collection">
            <input value={newCollection} onChange={(event) => setNewCollection(event.target.value)} onKeyDown={(event) => { if (event.key === "Enter") addCollection(); }} placeholder="Collection name" />
            <button onClick={addCollection} aria-label="Add collection"><Icon name="plus" /></button>
          </div>
          <div className="collection-list">
            {collections.length === 0 ? <div className="empty-sidebar"><Icon name="database" /><span>Add shortcuts for collections you use.</span></div> : collections.map((name) => (
              <div className="collection-item" key={name}>
                <button className="collection-name" onClick={() => chooseCollection(name)}><Icon name="database" /><span>{name}</span></button>
                <button className="remove-button" onClick={() => setCollections((items) => items.filter((item) => item !== name))} aria-label={`Remove ${name}`}><Icon name="trash" /></button>
              </div>
            ))}
          </div>
          <p className="sidebar-hint">Saved locally as navigation shortcuts. The v2 protocol does not expose collection discovery.</p>
        </section>

        <div className="sidebar-footer"><span>NovaQL native</span><span>Protocol v2</span></div>
      </aside>

      <main className="workspace">
        <section className="workspace-heading">
          <div><p className="eyebrow">Query workspace</p><h1>Explore with NovaQL</h1><p>Write document-native queries without relational mapping.</p></div>
          <div className="shortcut"><kbd>Ctrl</kbd><span>+</span><kbd>Enter</kbd><span>to run</span></div>
        </section>

        <section className="query-card">
          <div className="card-toolbar">
            <div className="toolbar-title"><Icon name="terminal" /><span>NovaQL editor</span><span className="unsaved-dot" /></div>
            <div className="toolbar-actions">
              <select aria-label="Query templates" defaultValue="" onChange={(event) => { const template = templates[Number(event.target.value)]; if (template) setQuery(template.query); event.target.value = ""; }}>
                <option value="" disabled>Insert template…</option>
                {templates.map((template, index) => <option value={index} key={template.label}>{template.label}</option>)}
              </select>
              <button className="secondary" onClick={explainQuery} disabled={running || !query.trim()}>Explain</button>
              <button className="primary run-button" onClick={() => void runQuery()} disabled={running || !query.trim()}><Icon name={running ? "clock" : "play"} />{running ? "Running…" : "Run query"}</button>
            </div>
          </div>
          <div className="editor-wrap">
            <div className="line-numbers" aria-hidden="true">{Array.from({ length: lineCount }, (_, index) => <span key={index}>{index + 1}</span>)}</div>
            <textarea ref={editorRef} value={query} onChange={(event) => setQuery(event.target.value)} spellCheck={false} aria-label="NovaQL query editor" />
          </div>
          <div className="editor-status"><span>{query.length.toLocaleString()} chars</span><span>UTF-8</span><span>NovaQL</span></div>
        </section>

        <section className={`result-card ${error ? "has-error" : ""}`}>
          <div className="card-toolbar result-toolbar">
            <div className="toolbar-title"><span className={`result-indicator ${error ? "error" : result ? "success" : "idle"}`} /> <span>{error ? "Query error" : result ? "Result" : "Result preview"}</span></div>
            <div className="result-meta">
              {result && <><span>Request #{result.requestId}</span><span>{result.elapsedMillis} ms</span></>}
              {error && <span>{error.kind || "Error"}</span>}
              <button className="icon-button small" onClick={() => void copyResult()} disabled={!result} aria-label="Copy result"><Icon name="copy" /></button>
              {copied && <span className="copied-label">Copied</span>}
            </div>
          </div>
          <div className="result-body">
            {error ? <div className="error-state"><strong>{error.kind || "Query failed"}</strong><p>{error.message || "The native client returned an unknown error."}</p></div> : result ? <pre>{result.payload}</pre> : <div className="empty-result"><div className="empty-orbit"><Icon name="database" /></div><strong>Your results will appear here</strong><p>Connect to NovaDB, then run a NovaQL statement.</p></div>}
          </div>
        </section>
      </main>

      <aside className="inspector">
        <section className="inspector-block">
          <div className="section-row"><div className="section-label">NovaQL patterns</div><span>{templates.length}</span></div>
          <div className="template-list">{templates.map((template) => <button key={template.label} onClick={() => setQuery(template.query)}><div><strong>{template.label}</strong><span>{template.description}</span></div><Icon name="chevron" /></button>)}</div>
        </section>
        <section className="inspector-block history-block">
          <div className="section-row"><div className="section-label">Session history</div>{history.length > 0 && <button className="clear-history" onClick={() => setHistory([])}>Clear</button>}</div>
          <div className="history-list">{history.length === 0 ? <p className="muted">Queries run in this window appear here. Query text is not persisted.</p> : history.map((item) => <button key={item.id} onClick={() => setQuery(item.query)}><span className={`history-status ${item.ok ? "ok" : "bad"}`} /><div><code>{item.query.replace(/\s+/g, " ").slice(0, 62)}</code><span>{item.timestamp.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })} · {item.elapsedMillis} ms</span></div></button>)}</div>
        </section>
        <section className="protocol-note"><Icon name="bolt" /><div><strong>Honest protocol boundary</strong><p>Results are shown exactly as returned by NovaDB protocol v2. Structured grids will arrive with a future typed-result protocol.</p></div></section>
      </aside>
    </div>
  );
}

export default App;
