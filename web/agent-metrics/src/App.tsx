import { useState } from "react";
import { getStoredToken } from "./api/client";
import AgentMetricsDashboard from "./components/AgentMetricsDashboard";
import LoginForm from "./components/LoginForm";

export default function App() {
  const [authenticated, setAuthenticated] = useState(Boolean(getStoredToken()));

  if (!authenticated) {
    return <LoginForm onSuccess={() => setAuthenticated(true)} />;
  }

  return <AgentMetricsDashboard onLogout={() => setAuthenticated(false)} />;
}
