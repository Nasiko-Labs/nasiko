import React, { useState, useCallback } from 'react';
import Sidebar from './components/Sidebar.jsx';
import UploadScreen from './components/UploadScreen.jsx';
import TraceDashboard from './components/TraceDashboard.jsx';
import MCPUploadScreen from './components/MCPUploadScreen.jsx';
import MCPServerList from './components/MCPServerList.jsx';

export default function App() {
  const [view, setView]           = useState('mcp-upload'); // Start with MCP upload for demo
  const [servers, setServers]     = useState([]);
  const [tools, setTools]         = useState([]);

  const handleDeployed = useCallback(({ server, tools: t }) => {
    setServers(prev => prev.find(s => s.name === server.name) ? prev : [...prev, server]);
    setTools(t);
  }, []);

  const handleNav = useCallback((v) => setView(v), []);
  const goToDashboard = useCallback(() => setView('dashboard'), []);

  return (
    <>
      <Sidebar view={view} onNav={handleNav} servers={servers} />
      {view === 'upload' && (
        <UploadScreen
          onServerDeployed={handleDeployed}
          onNavigateToDashboard={goToDashboard}
        />
      )}
      {view === 'mcp-upload' && (
        <MCPUploadScreen />
      )}
      {view === 'mcp-servers' && (
        <MCPServerList />
      )}
      {view === 'dashboard' && (
        <TraceDashboard servers={servers} tools={tools} />
      )}
    </>
  );
}
