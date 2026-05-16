import { FormEvent, useState } from "react";
import { useAuth } from "../context/AuthContext";
import { NasikoShell } from "./NasikoShell";

const LOGO_SRC = "/app/assets/assets/images/nasiko_logo.svg";

export function LoginPanel() {
  const { login } = useAuth();
  const [accessKey, setAccessKey] = useState("");
  const [accessSecret, setAccessSecret] = useState("");
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const handleSubmit = async (e: FormEvent) => {
    e.preventDefault();
    setLoading(true);
    setError(null);
    try {
      await login(accessKey.trim(), accessSecret);
    } catch (err) {
      setError(err instanceof Error ? err.message : "Login failed");
    } finally {
      setLoading(false);
    }
  };

  return (
    <NasikoShell pageTitle="Sign in" activeNav="logs">
      <div className="login-layout">
        <section className="card login-card">
          <div className="login-brand">
            <img src={LOGO_SRC} alt="Nasiko" width={48} height={48} />
            <h2>Sign in to Nasiko</h2>
            <p>Access platform logs and agent metrics</p>
          </div>
          <p className="hint">
            Use your access key and secret from{" "}
            <code>orchestrator/superuser_credentials.json</code>.
          </p>
          <form onSubmit={(e) => void handleSubmit(e)}>
            <label>
              Access key
              <input
                type="text"
                autoComplete="username"
                value={accessKey}
                onChange={(e) => setAccessKey(e.target.value)}
                placeholder="NASK_..."
                required
              />
            </label>
            <label>
              Access secret
              <input
                type="password"
                autoComplete="current-password"
                value={accessSecret}
                onChange={(e) => setAccessSecret(e.target.value)}
                required
              />
            </label>
            {error && <p className="form-error">{error}</p>}
            <button type="submit" className="btn primary" disabled={loading}>
              {loading ? "Signing in…" : "Continue"}
            </button>
          </form>
        </section>
      </div>
    </NasikoShell>
  );
}
