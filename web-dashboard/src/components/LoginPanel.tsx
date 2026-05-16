import { FormEvent, useState } from "react";

const LOGO_SRC = "/app/assets/assets/images/nasiko_logo.svg";

interface LoginPanelProps {
  onLogin: (accessKey: string, accessSecret: string) => Promise<void>;
}

export function LoginPanel({ onLogin }: LoginPanelProps) {
  const [accessKey, setAccessKey] = useState("");
  const [accessSecret, setAccessSecret] = useState("");
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const handleSubmit = async (e: FormEvent) => {
    e.preventDefault();
    setLoading(true);
    setError(null);
    try {
      await onLogin(accessKey.trim(), accessSecret);
    } catch (err) {
      setError(err instanceof Error ? err.message : "Login failed");
    } finally {
      setLoading(false);
    }
  };

  return (
    <div className="login-layout">
      <section className="card login-card">
        <div className="login-brand">
          <img src={LOGO_SRC} alt="Nasiko" width={48} height={48} />
          <h2>Sign in to Nasiko</h2>
          <p>View platform logs with your access credentials</p>
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
  );
}
