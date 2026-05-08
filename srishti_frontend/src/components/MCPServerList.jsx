import { useState, useEffect } from 'react';
import './MCPServerList.css';

export default function MCPServerList() {
  const [servers, setServers] = useState([]);
  const [loading, setLoading] = useState(true);
  const [selectedServer, setSelectedServer] = useState(null);
  const [searchQuery, setSearchQuery] = useState('');

  useEffect(() => {
    fetchMCPServers();
    // Poll for updates every 5 seconds
    const interval = setInterval(fetchMCPServers, 5000);
    return () => clearInterval(interval);
  }, []);

  const fetchMCPServers = async () => {
    try {
      const response = await fetch('http://localhost:8000/api/v1/registry/agents');
      if (response.ok) {
        const data = await response.json();
        // Filter only MCP servers
        const mcpServers = data.data?.filter(agent => 
          agent.artifactType === 'mcp_server' || 
          agent.name?.includes('mcp') ||
          agent.capabilities?.tools?.length > 0
        ) || [];
        setServers(mcpServers);
      }
    } catch (error) {
      console.error('Failed to fetch MCP servers:', error);
    } finally {
      setLoading(false);
    }
  };

  const filteredServers = servers.filter(server =>
    server.name?.toLowerCase().includes(searchQuery.toLowerCase()) ||
    server.description?.toLowerCase().includes(searchQuery.toLowerCase())
  );

  const getStatusColor = (status) => {
    if (status === 'running' || status === 'active') return '#10b981';
    if (status === 'error' || status === 'failed') return '#ef4444';
    return '#f59e0b';
  };

  const getStatusIcon = (status) => {
    if (status === 'running' || status === 'active') return '✓';
    if (status === 'error' || status === 'failed') return '✗';
    return '○';
  };

  return (
    <div className="mcp-server-list-container">
      <div className="list-header">
        <div className="header-content">
          <h1>🔧 MCP Server Registry</h1>
          <p>Manage and monitor your deployed MCP servers</p>
        </div>
        
        <div className="header-stats">
          <div className="stat-card">
            <div className="stat-value">{servers.length}</div>
            <div className="stat-label">Total Servers</div>
          </div>
          <div className="stat-card">
            <div className="stat-value">
              {servers.filter(s => s.status === 'running').length}
            </div>
            <div className="stat-label">Active</div>
          </div>
          <div className="stat-card">
            <div className="stat-value">
              {servers.reduce((sum, s) => sum + (s.capabilities?.tools?.length || 0), 0)}
            </div>
            <div className="stat-label">Total Tools</div>
          </div>
        </div>
      </div>

      <div className="search-bar">
        <input
          type="text"
          placeholder="🔍 Search MCP servers..."
          value={searchQuery}
          onChange={(e) => setSearchQuery(e.target.value)}
          className="search-input"
        />
      </div>

      {loading ? (
        <div className="loading-state">
          <div className="spinner-large"></div>
          <p>Loading MCP servers...</p>
        </div>
      ) : filteredServers.length === 0 ? (
        <div className="empty-state">
          <div className="empty-icon">📦</div>
          <h3>No MCP Servers Found</h3>
          <p>Upload your first MCP server to get started</p>
        </div>
      ) : (
        <div className="servers-grid">
          {filteredServers.map((server, idx) => (
            <div 
              key={idx} 
              className="server-card"
              onClick={() => setSelectedServer(server)}
            >
              <div className="card-header">
                <div className="server-name">
                  <span className="server-icon">🔧</span>
                  {server.name || 'Unnamed Server'}
                </div>
                <div 
                  className="status-badge"
                  style={{ 
                    background: `${getStatusColor(server.status)}20`,
                    color: getStatusColor(server.status)
                  }}
                >
                  <span className="status-icon">{getStatusIcon(server.status)}</span>
                  {server.status || 'unknown'}
                </div>
              </div>

              <div className="card-description">
                {server.description || 'No description available'}
              </div>

              <div className="card-stats">
                <div className="stat-item">
                  <span className="stat-icon">🔧</span>
                  <span className="stat-text">
                    {server.capabilities?.tools?.length || 0} tools
                  </span>
                </div>
                <div className="stat-item">
                  <span className="stat-icon">📡</span>
                  <span className="stat-text">
                    {server.transport || 'stdio'}
                  </span>
                </div>
                <div className="stat-item">
                  <span className="stat-icon">⚡</span>
                  <span className="stat-text">
                    {server.invocations || 0} calls
                  </span>
                </div>
              </div>

              <div className="card-footer">
                <div className="server-url">
                  {server.url || 'No URL'}
                </div>
                <button className="test-button">Test</button>
              </div>
            </div>
          ))}
        </div>
      )}

      {/* Server Detail Modal */}
      {selectedServer && (
        <div className="modal-overlay" onClick={() => setSelectedServer(null)}>
          <div className="modal-content" onClick={(e) => e.stopPropagation()}>
            <div className="modal-header">
              <h2>🔧 {selectedServer.name}</h2>
              <button 
                className="close-button"
                onClick={() => setSelectedServer(null)}
              >
                ✕
              </button>
            </div>

            <div className="modal-body">
              <div className="detail-section">
                <h3>Server Information</h3>
                <div className="detail-grid">
                  <div className="detail-item">
                    <span className="detail-label">Status:</span>
                    <span className="detail-value">{selectedServer.status}</span>
                  </div>
                  <div className="detail-item">
                    <span className="detail-label">Version:</span>
                    <span className="detail-value">{selectedServer.version || '1.0.0'}</span>
                  </div>
                  <div className="detail-item">
                    <span className="detail-label">Transport:</span>
                    <span className="detail-value">{selectedServer.transport || 'stdio'}</span>
                  </div>
                  <div className="detail-item">
                    <span className="detail-label">URL:</span>
                    <span className="detail-value url">{selectedServer.url}</span>
                  </div>
                </div>
              </div>

              <div className="detail-section">
                <h3>Available Tools ({selectedServer.capabilities?.tools?.length || 0})</h3>
                <div className="tools-list">
                  {selectedServer.capabilities?.tools?.map((tool, idx) => (
                    <div key={idx} className="tool-detail-card">
                      <div className="tool-header">
                        <span className="tool-name">{tool.name}</span>
                        <button className="invoke-button">Invoke</button>
                      </div>
                      <div className="tool-description">
                        {tool.description || 'No description'}
                      </div>
                      {tool.inputSchema && (
                        <div className="tool-schema">
                          <span className="schema-label">Input Schema:</span>
                          <pre>{JSON.stringify(tool.inputSchema, null, 2)}</pre>
                        </div>
                      )}
                    </div>
                  ))}
                </div>
              </div>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
