import { StrictMode, useState, type FormEvent } from "react";
import { createRoot } from "react-dom/client";
import { BrowserRouter, Link, NavLink, Route, Routes, useNavigate } from "react-router-dom";
import { type Found, get } from "./api";
import AddressPage from "./pages/Address";
import ChecksPage from "./pages/Checks";
import ConfigPage from "./pages/Config";
import EpochPage from "./pages/Epoch";
import OverlaySetupPage from "./pages/OverlaySetup";
import OverviewPage from "./pages/Overview";
import ReceiptPage from "./pages/Receipt";
import "./styles.css";

function Search() {
  const navigate = useNavigate();
  const [query, setQuery] = useState("");
  const [message, setMessage] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  async function submit(event: FormEvent) {
    event.preventDefault();
    const q = query.trim();
    if (!q) return;
    setBusy(true);
    setMessage(null);
    try {
      const found = await get<Found>(`/search?q=${encodeURIComponent(q)}`);
      if (found.kind === "transaction") navigate(`/tx/${found.signature}`);
      else if (found.kind === "config") navigate(`/config/${found.config_id}`);
      else if (found.kind === "address") navigate(`/address/${found.address}`);
      else setMessage("No transaction, config or address matches that.");
    } catch {
      setMessage("Search is unavailable right now. Try again.");
    } finally {
      setBusy(false);
    }
  }

  return (
    <form className="search" role="search" onSubmit={submit}>
      <label htmlFor="search" className="visually-hidden">
        Search by transaction signature, config ID or address
      </label>
      <input
        id="search"
        type="search"
        placeholder="Signature, config ID or address"
        value={query}
        onChange={(e) => setQuery(e.target.value)}
        autoComplete="off"
        spellCheck={false}
        aria-describedby={message ? "search-message" : undefined}
      />
      <button type="submit" className="button" disabled={busy}>
        {busy ? "Searching" : "Search"}
      </button>
      {message && (
        <p id="search-message" className="search-message" role="status">
          {message}
        </p>
      )}
    </form>
  );
}

function NotFoundPage() {
  return (
    <div className="notice">
      <h1>Page not found</h1>
      <p>
        <Link to="/">Go to the overview</Link>
      </p>
    </div>
  );
}

function App() {
  return (
    <BrowserRouter>
      <a href="#main" className="skip">
        Skip to content
      </a>
      <header className="header">
        <div className="header-inner">
          <Link to="/" className="brand" aria-label="Relayscope overview">
            <span className="brand-mark" aria-hidden="true" />
            Relayscope
          </Link>
          <nav aria-label="Main">
            <NavLink to="/" end>
              Overview
            </NavLink>
            <NavLink to="/checks">Checks</NavLink>
          </nav>
          <Search />
        </div>
      </header>
      <main id="main" className="main">
        <Routes>
          <Route path="/" element={<OverviewPage />} />
          <Route path="/tx/:signature" element={<ReceiptPage />} />
          <Route path="/epoch/:config/:mint/:epoch" element={<EpochPage />} />
          <Route path="/config/:config" element={<ConfigPage />} />
          <Route path="/config/:config/overlay" element={<OverlaySetupPage />} />
          <Route path="/address/:address" element={<AddressPage />} />
          <Route path="/checks" element={<ChecksPage />} />
          <Route path="*" element={<NotFoundPage />} />
        </Routes>
      </main>
      <footer className="footer">
        <p>
          Relayscope is an independent, unofficial tracker for the Donation Relay program on Solana (
          <span className="mono">RLAYHr9T…7RZu</span>). Every amount comes from a transaction you can open on Solana Explorer.
        </p>
      </footer>
    </BrowserRouter>
  );
}

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
