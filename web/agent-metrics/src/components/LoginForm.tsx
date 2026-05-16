import { FormEvent, useState } from "react";
import { login } from "../api/client";

interface LoginFormProps {
  onSuccess: () => void;
}

export default function LoginForm({ onSuccess }: LoginFormProps) {
  const [accessKey, setAccessKey] = useState("");
  const [accessSecret, setAccessSecret] = useState("");
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function handleSubmit(event: FormEvent) {
    event.preventDefault();
    setLoading(true);
    setError(null);
    try {
      await login(accessKey.trim(), accessSecret.trim());
      onSuccess();
    } catch (err) {
      setError(err instanceof Error ? err.message : "Login failed");
    } finally {
      setLoading(false);
    }
  }

  return (
    <div className="panel login-panel">
      <h2>Sign in to Nasiko</h2>
      <p style={{ color: "var(--muted)", marginTop: 0 }}>
        Use the access key and secret from{" "}
        <code>orchestrator/superuser_credentials.json</code>.
      </p>
      {error && <div className="error-banner">{error}</div>}
      <form onSubmit={handleSubmit}>
        <div className="field">
          <label htmlFor="access-key">Access key</label>
          <input
            id="access-key"
            value={accessKey}
            onChange={(e) => setAccessKey(e.target.value)}
            autoComplete="username"
            required
          />
        </div>
        <div className="field">
          <label htmlFor="access-secret">Access secret</label>
          <input
            id="access-secret"
            type="password"
            value={accessSecret}
            onChange={(e) => setAccessSecret(e.target.value)}
            autoComplete="current-password"
            required
          />
        </div>
        <button className="btn btn-primary" type="submit" disabled={loading}>
          {loading ? "Signing in…" : "Sign in"}
        </button>
      </form>
    </div>
  );
}
