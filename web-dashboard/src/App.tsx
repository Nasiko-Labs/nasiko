import { Navigate, Route, Routes } from "react-router-dom";
import { AuthProvider, useAuth } from "./context/AuthContext";
import { LoginPanel } from "./components/LoginPanel";
import { LogsPage } from "./pages/LogsPage";
import { MetricsPage } from "./pages/MetricsPage";

function RequireAuth({ children }: { children: React.ReactNode }) {
  const { token } = useAuth();
  if (!token) {
    return <LoginPanel />;
  }
  return <>{children}</>;
}

function AppRoutes() {
  return (
    <Routes>
      <Route path="/" element={<Navigate to="/logs/" replace />} />
      <Route
        path="/logs/*"
        element={
          <RequireAuth>
            <LogsPage />
          </RequireAuth>
        }
      />
      <Route
        path="/metrics/*"
        element={
          <RequireAuth>
            <MetricsPage />
          </RequireAuth>
        }
      />
      <Route path="*" element={<Navigate to="/logs/" replace />} />
    </Routes>
  );
}

export default function App() {
  return (
    <AuthProvider>
      <AppRoutes />
    </AuthProvider>
  );
}
