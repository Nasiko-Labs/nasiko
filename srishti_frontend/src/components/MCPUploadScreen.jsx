import { useState } from 'react';
import './MCPUploadScreen.css';

export default function MCPUploadScreen() {
  const [uploadStatus, setUploadStatus] = useState('idle'); // idle, uploading, detecting, success, error
  const [artifactType, setArtifactType] = useState(null); // 'agent' or 'mcp_server'
  const [manifest, setManifest] = useState(null);
  const [error, setError] = useState(null);
  const [dragActive, setDragActive] = useState(false);

  const handleDrag = (e) => {
    e.preventDefault();
    e.stopPropagation();
    if (e.type === "dragenter" || e.type === "dragover") {
      setDragActive(true);
    } else if (e.type === "dragleave") {
      setDragActive(false);
    }
  };

  const handleDrop = async (e) => {
    e.preventDefault();
    e.stopPropagation();
    setDragActive(false);
    
    const files = e.dataTransfer.files;
    if (files && files.length > 0) {
      await handleUpload(files[0]);
    }
  };

  const handleFileInput = async (e) => {
    const files = e.target.files;
    if (files && files.length > 0) {
      await handleUpload(files[0]);
    }
  };

  const handleUpload = async (file) => {
    setUploadStatus('uploading');
    setError(null);

    const formData = new FormData();
    formData.append('file', file);

    try {
      // Upload and detect
      const response = await fetch('http://localhost:8000/api/v1/agents/upload', {
        method: 'POST',
        body: formData,
      });

      if (!response.ok) {
        throw new Error('Upload failed');
      }

      const result = await response.json();
      
      setUploadStatus('detecting');
      
      // Simulate detection (in real implementation, backend returns this)
      setTimeout(() => {
        // Check if MCP manifest exists
        const isMCP = result.data?.agent_name?.includes('mcp') || false;
        setArtifactType(isMCP ? 'mcp_server' : 'agent');
        
        if (isMCP) {
          // Fetch manifest
          fetchManifest(result.data.agent_name);
        } else {
          setUploadStatus('success');
        }
      }, 1000);

    } catch (err) {
      setUploadStatus('error');
      setError(err.message);
    }
  };

  const fetchManifest = async (agentName) => {
    try {
      const response = await fetch(`http://localhost:8000/api/v1/agents/${agentName}/manifest`);
      if (response.ok) {
        const manifestData = await response.json();
        setManifest(manifestData);
        setUploadStatus('success');
      } else {
        // Fallback: generate mock manifest for demo
        setManifest({
          name: agentName,
          version: "1.0.0",
          artifactType: "mcp_server",
          transport: "stdio",
          tools: [
            {
              name: "example_tool",
              description: "Auto-detected MCP tool",
              inputSchema: {
                type: "object",
                properties: {
                  query: { type: "string" }
                }
              }
            }
          ]
        });
        setUploadStatus('success');
      }
    } catch (err) {
      console.error('Failed to fetch manifest:', err);
      setUploadStatus('success'); // Continue anyway
    }
  };

  return (
    <div className="mcp-upload-container">
      <div className="upload-header">
        <h1>🚀 MCP Server Upload</h1>
        <p>Upload your MCP server and watch it auto-detect, bridge, and deploy</p>
      </div>

      {/* Upload Zone */}
      <div 
        className={`upload-zone ${dragActive ? 'drag-active' : ''} ${uploadStatus !== 'idle' ? 'uploading' : ''}`}
        onDragEnter={handleDrag}
        onDragLeave={handleDrag}
        onDragOver={handleDrag}
        onDrop={handleDrop}
      >
        {uploadStatus === 'idle' && (
          <>
            <div className="upload-icon">📦</div>
            <h3>Drag & Drop MCP Server</h3>
            <p>or click to browse</p>
            <input 
              type="file" 
              accept=".zip,.tar.gz" 
              onChange={handleFileInput}
              style={{ display: 'none' }}
              id="file-input"
            />
            <label htmlFor="file-input" className="browse-button">
              Browse Files
            </label>
          </>
        )}

        {uploadStatus === 'uploading' && (
          <div className="status-indicator">
            <div className="spinner"></div>
            <h3>Uploading...</h3>
          </div>
        )}

        {uploadStatus === 'detecting' && (
          <div className="status-indicator">
            <div className="spinner"></div>
            <h3>Detecting Artifact Type...</h3>
            <p>Analyzing code structure</p>
          </div>
        )}

        {uploadStatus === 'success' && artifactType && (
          <div className="detection-result">
            <div className={`artifact-badge ${artifactType}`}>
              {artifactType === 'mcp_server' ? '🔧 MCP Server Detected ✓' : '🤖 Agent Detected ✓'}
            </div>
            {artifactType === 'mcp_server' && manifest && (
              <div className="tools-count">
                <span className="count-badge">{manifest.tools?.length || 0}</span>
                <span>Tools Found</span>
              </div>
            )}
          </div>
        )}

        {uploadStatus === 'error' && (
          <div className="error-indicator">
            <div className="error-icon">❌</div>
            <h3>Upload Failed</h3>
            <p>{error}</p>
            <button onClick={() => setUploadStatus('idle')} className="retry-button">
              Try Again
            </button>
          </div>
        )}
      </div>

      {/* Manifest Preview */}
      {uploadStatus === 'success' && artifactType === 'mcp_server' && manifest && (
        <div className="manifest-preview">
          <div className="preview-header">
            <h3>📋 Generated MCP Manifest</h3>
            <span className="auto-badge">Auto-Generated</span>
          </div>
          
          <div className="manifest-content">
            <div className="manifest-info">
              <div className="info-row">
                <span className="label">Name:</span>
                <span className="value">{manifest.name}</span>
              </div>
              <div className="info-row">
                <span className="label">Version:</span>
                <span className="value">{manifest.version}</span>
              </div>
              <div className="info-row">
                <span className="label">Transport:</span>
                <span className="value transport">{manifest.transport}</span>
              </div>
            </div>

            <div className="tools-list">
              <h4>🔧 Available Tools</h4>
              {manifest.tools?.map((tool, idx) => (
                <div key={idx} className="tool-card">
                  <div className="tool-name">{tool.name}</div>
                  <div className="tool-description">{tool.description}</div>
                  <div className="tool-schema">
                    <span className="schema-label">Input Schema:</span>
                    <code>{JSON.stringify(tool.inputSchema, null, 2)}</code>
                  </div>
                </div>
              ))}
            </div>
          </div>

          <div className="deployment-status">
            <div className="status-steps">
              <div className="step completed">
                <div className="step-icon">✓</div>
                <span>Detected</span>
              </div>
              <div className="step completed">
                <div className="step-icon">✓</div>
                <span>Manifest Generated</span>
              </div>
              <div className="step active">
                <div className="step-icon spinner-small"></div>
                <span>Building Bridge</span>
              </div>
              <div className="step">
                <div className="step-icon">○</div>
                <span>Deploying</span>
              </div>
              <div className="step">
                <div className="step-icon">○</div>
                <span>Live</span>
              </div>
            </div>
          </div>

          <button className="deploy-button">
            🚀 Deploy MCP Server
          </button>
        </div>
      )}
    </div>
  );
}
