import React, { useState, useCallback, useRef, useEffect } from 'react';
import { Upload, FileCode, CheckCircle, AlertTriangle, ArrowRight, Wrench, Zap, Search } from 'lucide-react';
import { MCP_SERVER, MANIFEST, DEPLOY_STEPS } from '../simulation/mockData.js';
import { simulateUpload } from '../simulation/engine.js';

const BEFORE_ITEMS = [
  'Write manifest manually (2–4 hours)',
  'Configure routing and bridge code',
  'Restart agents and redeploy containers',
  'Debug failures blindly — no visibility',
];

const AFTER_ITEMS = [
  'Auto-detection in seconds',
  'Zero-config bridge setup',
  'Live tool discovery — instant',
  'Full observability from day one',
];

const TOOL_ICONS = { analyze_failure_logs: '🔍', predict_failure_chain: '⚡', suggest_prevention_fix: '🛠' };

export default function UploadScreen({ onServerDeployed, onNavigateToDashboard }) {
  const [phase, setPhase] = useState('idle'); // idle | uploading | complete
  const [steps, setSteps] = useState([]);     // { status: 'pending'|'active'|'done' }
  const inputRef = useRef();

  const startDeploy = useCallback(async () => {
    if (phase !== 'idle') return;
    setPhase('uploading');

    const stepStates = DEPLOY_STEPS.map(() => 'pending');
    setSteps([...stepStates]);

    await simulateUpload(({ index, status }) => {
      setSteps(prev => {
        const next = [...prev];
        next[index] = status;
        return next;
      });
    });

    setPhase('complete');
    onServerDeployed({ server: MCP_SERVER, tools: MCP_SERVER.tools });
  }, [phase, onServerDeployed]);

  const handleDrop = useCallback((e) => {
    e.preventDefault();
    startDeploy();
  }, [startDeploy]);

  const manifestStr = JSON.stringify(MANIFEST, null, 2);

  return (
    <div className="main-content">
      <div className="upload-screen">
        {/* Hero */}
        <div className="upload-hero">
          <div className="upload-badge">
            <Zap size={10} /> MCP Server Publishing
          </div>
          <h1 className="upload-title">
            <span>Nasiko MCP Platform</span><br />Zero-Config Agent Integration
          </h1>
          <p className="upload-subtitle">
            Upload any MCP server. Nasiko auto-detects, generates manifests,
            bridges stdio→HTTP, and makes tools instantly discoverable with full observability.
          </p>
        </div>

        {/* Before / After */}
        <div className="before-after">
          <div className="before-card">
            <div className="ba-header">
              <AlertTriangle size={13} style={{ color: '#ef4444' }} />
              Manual MCP Integration
            </div>
            <div className="ba-list">
              {BEFORE_ITEMS.map((item, i) => (
                <div key={i} className="ba-item">
                  <span className="ba-dot-red" />
                  {item}
                </div>
              ))}
            </div>
          </div>
          <div className="after-card">
            <div className="ba-header" style={{ color: 'var(--success)' }}>
              <CheckCircle size={13} />
              Nasiko Automated
            </div>
            <div className="ba-list">
              {AFTER_ITEMS.map((item, i) => (
                <div key={i} className="ba-item">
                  <span className="ba-dot-green" />
                  {item}
                </div>
              ))}
            </div>
          </div>
        </div>

        {/* Drop Zone */}
        {phase === 'idle' && (
          <div
            className="dropzone"
            onClick={startDeploy}
            onDragOver={e => e.preventDefault()}
            onDrop={handleDrop}
          >
            <div className="dropzone-icon">
              <Upload size={24} />
            </div>
            <div className="dropzone-title">Drop MCP server files here</div>
            <div className="dropzone-sub">or click to select · Python / Node.js</div>
            <div className="file-chip">
              <FileCode size={12} />
              failure_analysis_server.py
            </div>
            <input ref={inputRef} type="file" style={{ display: 'none' }} />
          </div>
        )}

        {/* Deployment Progress */}
        {(phase === 'uploading' || phase === 'complete') && (
          <div className="deploy-steps">
            <div className="card-header" style={{ marginBottom: 16 }}>
              <span className="card-title">
                <FileCode size={12} /> Deployment Progress
              </span>
              {phase === 'complete' && (
                <span className="badge badge-green">
                  <CheckCircle size={9} /> Complete
                </span>
              )}
            </div>

            {DEPLOY_STEPS.map((step, i) => {
              const status = steps[i] || 'pending';
              return (
                <div key={step.id} className="deploy-step">
                  <div className={`deploy-step-icon ${status === 'done' ? 'step-done' : status === 'active' ? 'step-active' : 'step-pending'}`}>
                    {status === 'done' ? '✓' : status === 'active' ? '…' : String(i + 1)}
                  </div>
                  <div className="deploy-step-text">
                    <strong>{step.label}</strong>
                    {status !== 'pending' && (
                      <span style={{ color: 'var(--text-muted)', marginLeft: 8, fontSize: 11, fontFamily: 'var(--font-mono)' }}>
                        {step.detail}
                      </span>
                    )}
                  </div>
                </div>
              );
            })}

            {phase === 'complete' && (
              <div className="deploy-live">✓ Server is LIVE — AI Failure Analysis tools are discoverable</div>
            )}
          </div>
        )}

        {/* Manifest Preview */}
        {phase === 'complete' && (
          <div className="manifest-preview">
            <div className="card-header" style={{ marginBottom: 12 }}>
              <span className="card-title"><FileCode size={12} /> Generated Manifest</span>
              <span className="badge badge-blue">AgentCard.json · MCP Protocol</span>
            </div>
            <div style={{ fontSize: 11, color: 'var(--text-muted)', marginBottom: 10, lineHeight: 1.5 }}>
              This manifest is the <strong style={{ color: 'var(--text-primary)' }}>contract between the MCP server and any connected agent</strong>.
              Nasiko generates it automatically from the uploaded server — no manual configuration.
            </div>
            <pre className="manifest-code">{manifestStr}</pre>
          </div>
        )}

        {/* Discovery Protocol Exchange */}
        {phase === 'complete' && <DiscoveryProtocol />}

        {/* Discovered Tools */}
        {phase === 'complete' && (
          <div className="tools-discovered">
            <div className="card-header" style={{ marginBottom: 16 }}>
              <span className="card-title"><Search size={12} /> Tools Registered in Agent Inventory</span>
              <span className="badge badge-green">{MCP_SERVER.tools.length} tools · No restart required</span>
            </div>
            {MCP_SERVER.tools.map(tool => (
              <div key={tool.id} className="discovered-tool-item">
                <div className="tool-icon-box">{tool.icon}</div>
                <div style={{ flex: 1, minWidth: 0 }}>
                  <div className="tool-item-name">{tool.name}</div>
                  <div className="tool-item-desc">{tool.description}</div>
                </div>
                <span className="badge badge-new">NEW</span>
              </div>
            ))}
          </div>
        )}

        {/* Zero-code proof callout */}
        {phase === 'complete' && (
          <div style={{
            background: 'rgba(34,197,94,0.06)',
            border: '1px solid rgba(34,197,94,0.2)',
            borderRadius: 'var(--radius-lg)',
            padding: '14px 20px',
            display: 'flex',
            alignItems: 'center',
            gap: 12,
            marginBottom: 16,
          }}>
            <CheckCircle size={16} style={{ color: 'var(--success)', flexShrink: 0 }} />
            <div>
              <div style={{ fontSize: 13, fontWeight: 600, color: 'var(--text-primary)' }}>
                Zero code changes. Zero agent restart.
              </div>
              <div style={{ fontSize: 11, color: 'var(--text-muted)', marginTop: 2 }}>
                The running LangChain agent automatically received the new tools via Nasiko's discovery service.
                No <code style={{ color: 'var(--primary)', fontSize: 10 }}>agent.add_tool()</code> calls.
                No redeployment.
              </div>
            </div>
          </div>
        )}

        {/* CTA */}
        {phase === 'complete' && (
          <button className="open-dashboard-btn" onClick={onNavigateToDashboard}>
            Open Dashboard — See Agent Use These Tools <ArrowRight size={16} />
          </button>
        )}
      </div>
    </div>
  );
}

function DiscoveryProtocol() {
  const code = `// 1. Agent queries Nasiko Gateway for available tools
GET /mcp/discover HTTP/1.1
Host: gateway.nasiko.internal

// 2. Gateway responds with MCP Server definitions
HTTP/1.1 200 OK
{
  "servers": [{
    "name": "failure-analysis",
    "tools": ["analyze_failure_logs", "predict_failure_chain", "suggest_prevention_fix"]
  }]
}

// 3. Agent auto-binds tools without restart
[Agent LOG] -> Bound new MCP tools correctly. Capability expanded.`;

  return (
    <div className="manifest-preview" style={{ marginTop: -8 }}>
      <div className="card-header" style={{ marginBottom: 12 }}>
        <span className="card-title" style={{ color: 'var(--success)' }}>
          <Zap size={12} /> Live Discovery Exchange
        </span>
        <span className="badge badge-gray mono">HTTP / JSON</span>
      </div>
      <div style={{ fontSize: 11, color: 'var(--text-muted)', marginBottom: 10, lineHeight: 1.5 }}>
        Under the hood, the agent periodically polls the discovery endpoint. When the MCP server deployed, it intercepted this and learned the new schema.
      </div>
      <pre className="manifest-code" style={{ background: '#0a0a0c', border: '1px solid rgba(67,70,85,0.3)', color: '#a0a2b0' }}>
        {code}
      </pre>
    </div>
  );
}
