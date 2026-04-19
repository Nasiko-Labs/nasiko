import React from 'react';
import { Upload, LayoutDashboard, Server, Activity, Cpu, Zap } from 'lucide-react';

const NAV = [
  { id: 'mcp-upload',    label: '🚀 Upload MCP Server', icon: Upload },
  { id: 'mcp-servers',   label: '🔧 MCP Registry', icon: Server },
  { id: 'dashboard',     label: '📊 Live Traces', icon: Activity },
  { id: 'upload',        label: '📤 Legacy Upload', icon: Cpu },
];

export default function Sidebar({ view, onNav, servers }) {
  return (
    <aside className="sidebar">
      <div className="sidebar-logo">
        <div className="logo-mark">N</div>
        <div className="logo-text">
          <span className="logo-name">Nasiko</span>
          <span className="logo-sub">MCP Platform</span>
        </div>
      </div>

      <nav className="nav-section">
        <span className="nav-label">Platform</span>
        {NAV.map(({ id, label, icon: Icon }) => (
          <button
            key={id}
            className={`nav-item${view === id ? ' active' : ''}`}
            onClick={() => onNav(id)}
          >
            <Icon size={14} />
            {label}
            {id === 'dashboard' && servers.length > 0 && (
              <span className="nav-badge-count">{servers.length}</span>
            )}
          </button>
        ))}
      </nav>

      {servers.length > 0 && (
        <div className="sidebar-servers">
          <span className="nav-label" style={{ display: 'flex', alignItems: 'center', gap: 6 }}>
            <Server size={10} /> Active Servers
          </span>
          {servers.map(s => (
            <div key={s.name} className="server-entry">
              <span className="server-dot" />
              <div>
                <div className="server-name">{s.displayName}</div>
                <div className="server-meta">{s.tools.length} tools</div>
              </div>
            </div>
          ))}
        </div>
      )}

      <div className="sidebar-footer">
        <div className="status-row">
          <Activity size={11} style={{ color: 'var(--success)' }} />
          <span>System Online</span>
          <span className="status-dot" style={{ marginLeft: 'auto' }} />
        </div>
        <div className="sidebar-version">v2.1.0 · Hackathon Build</div>
      </div>
    </aside>
  );
}
