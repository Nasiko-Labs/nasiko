import React, { useState, useCallback, useRef, useEffect } from 'react';
import { Send, Cpu, Wrench, ChevronDown, Code2, AlertCircle } from 'lucide-react';
import { DEMO_QUERIES } from '../simulation/mockData.js';
import { callAnalyze } from '../simulation/api.js';

const TOOL_ICONS = {
  analyze_failure_logs:  '🔍',
  predict_failure_chain: '⚡',
  suggest_prevention_fix: '🛠',
};

function pickTool(text) {
  const t = text.toLowerCase();
  if (t.includes('predict') || t.includes('chain') || t.includes('cascade')) return 'predict_failure_chain';
  if (t.includes('fix') || t.includes('suggest') || t.includes('prevent') || t.includes('remediat')) return 'suggest_prevention_fix';
  return 'analyze_failure_logs';
}

function severityColor(s) {
  if (s === 'CRITICAL') return '#ef4444';
  if (s === 'HIGH')     return '#d97706';
  return '#64748b';
}

export default function AgentPanel({ tools, onQueryStart, onQueryComplete, isRunning }) {
  const [input, setInput]         = useState('');
  const [messages, setMessages]   = useState([]);
  const [thinking, setThinking]   = useState(false);
  const [backendOk, setBackendOk] = useState(true);
  const chatRef = useRef(null);

  useEffect(() => {
    if (chatRef.current) chatRef.current.scrollTop = chatRef.current.scrollHeight;
  }, [messages, thinking]);

  const sendQuery = useCallback(async (text) => {
    if (!text.trim() || isRunning || thinking) return;

    const tool = pickTool(text);
    setMessages(prev => [...prev, { role: 'user', text, id: Date.now() }]);
    setInput('');
    setThinking(true);
    onQueryStart(tool);

    let result = null;
    try {
      result = await callAnalyze({ query: text, tool });
      setBackendOk(true);
    } catch (err) {
      // Graceful fallback to mock if backend down
      setBackendOk(false);
      const fallback = DEMO_QUERIES.find(q => q.tool === tool) || DEMO_QUERIES[0];
      result = {
        trace_id:   fallback.traceId,
        tool,
        latency_ms: fallback.totalMs,
        analysis: {
          failure_type:        'AnalysisEngine',
          severity:            'HIGH',
          confidence:          0.89,
          root_cause:          fallback.result.rootCause,
          event_count:         3,
          cascade_probability: 0.82,
          mttr_estimate_mins:  42,
          affected_services:   fallback.result.affectedServices,
          findings:            fallback.result.findings.map(f => ({
            service: f.title.split(' ')[0], signal: f.title, impact: 'HIGH',
          })),
        },
        remediation: fallback.result.findings.map((f, i) => ({
          action: f.title, priority: `P${i}`, eta_mins: 30 + i * 60,
        })),
        trace_spans: [],
      };
    }

    setThinking(false);
    setMessages(prev => [...prev, {
      role:    'agent',
      tool,
      result,
      rawJson: JSON.stringify(result, null, 2),
      id:      Date.now() + 1,
    }]);
    onQueryComplete(result);
  }, [isRunning, thinking, onQueryStart, onQueryComplete]);

  const handleKeyDown = (e) => {
    if (e.key === 'Enter' && !e.shiftKey) { e.preventDefault(); sendQuery(input); }
  };

  return (
    <div className="card" style={{ display: 'flex', flexDirection: 'column', height: '100%', padding: 20, gap: 0 }}>
      {/* Header */}
      <div className="card-header">
        <span className="card-title"><Cpu size={12} /> Agent Console</span>
        <div style={{ display: 'flex', alignItems: 'center', gap: 6 }}>
          {!backendOk && (
            <span className="badge badge-amber" title="Backend offline — using cached data">
              <AlertCircle size={8} /> Cached
            </span>
          )}
          {backendOk && (
            <span className="badge badge-green" style={{ fontSize: 9 }}>● Live API</span>
          )}
          <span className="badge badge-blue">{tools.length} tools</span>
        </div>
      </div>

      {/* Tool chips */}
      <div className="tool-chips">
        {tools.length === 0 ? (
          <span className="tool-chip">basic_agent_tool</span>
        ) : tools.map(t => (
          <span key={t.id} className="tool-chip new-tool">
            {TOOL_ICONS[t.id] || '🔧'} {t.name}
            <span className="badge badge-new" style={{ fontSize: 8, padding: '1px 5px' }}>NEW</span>
          </span>
        ))}
      </div>

      {/* Chat */}
      <div className="chat-area" ref={chatRef}>
        {messages.length === 0 && (
          <div style={{ color: 'var(--text-dim)', fontSize: 12, padding: '8px 0', textAlign: 'center' }}>
            Ask the agent to analyze any failure…
          </div>
        )}
        {messages.map(msg =>
          msg.role === 'user'
            ? <UserMsg key={msg.id} msg={msg} />
            : <AgentMsg key={msg.id} msg={msg} />
        )}
        {thinking && <ThinkingIndicator />}
      </div>

      {/* Suggestions */}
      {messages.length === 0 && (
        <div className="suggestions">
          {DEMO_QUERIES.map(q => (
            <button key={q.id} className="suggestion-btn" onClick={() => sendQuery(q.text)}>
              {q.text}
            </button>
          ))}
        </div>
      )}

      {/* Input */}
      <div className="chat-input-row">
        <input
          className="chat-input"
          value={input}
          onChange={e => setInput(e.target.value)}
          onKeyDown={handleKeyDown}
          placeholder="Describe a failure (memory, CPU, network, crash…)"
          disabled={thinking}
        />
        <button className="send-btn" onClick={() => sendQuery(input)} disabled={!input.trim() || thinking}>
          <Send size={13} />
        </button>
      </div>
    </div>
  );
}

function UserMsg({ msg }) {
  return (
    <div className="chat-msg">
      <div className="chat-msg-user">{msg.text}</div>
    </div>
  );
}

function AgentMsg({ msg }) {
  const [showRaw, setShowRaw] = useState(false);
  const r = msg.result;
  const a = r.analysis;

  return (
    <div className="chat-msg">
      <div className="chat-msg-agent">
        {/* Tool call indicator */}
        <div className="tool-call-indicator">
          <Wrench size={9} />
          Used: <span className="tool-name">{msg.tool}</span>
          <span className="badge badge-amber" style={{ fontSize: 8 }}>{r.latency_ms}ms</span>
          <span className="mono" style={{ fontSize: 9, color: 'var(--text-dim)', marginLeft: 2 }}>
            [{r.trace_id}]
          </span>
        </div>

        {/* Severity + confidence */}
        <div style={{ display: 'flex', alignItems: 'center', gap: 6, marginBottom: 8 }}>
          <span style={{
            fontSize: 10, fontWeight: 700, padding: '2px 7px', borderRadius: 4,
            background: `${severityColor(a.severity)}22`,
            color: severityColor(a.severity),
          }}>{a.severity}</span>
          <span style={{ fontSize: 10, color: 'var(--text-muted)' }}>
            {a.failure_type} · {Math.round(a.confidence * 100)}% confidence
          </span>
        </div>

        {/* Root cause */}
        <div style={{ fontSize: 12, color: 'var(--text-primary)', marginBottom: 10, lineHeight: 1.5 }}>
          {a.root_cause}
        </div>

        {/* Findings */}
        {a.findings?.map((f, i) => (
          <div key={i} style={{
            display: 'flex', alignItems: 'flex-start', gap: 6,
            padding: '5px 0', borderTop: '1px solid rgba(67,70,85,0.12)', fontSize: 11,
          }}>
            <span style={{ color: f.impact === 'HIGH' ? '#d97706' : 'var(--success)', marginTop: 2, fontSize: 8 }}>●</span>
            <div>
              <span style={{ color: 'var(--text-primary)', fontWeight: 500 }}>{f.signal || f.service}</span>
              {f.impact && (
                <span style={{ marginLeft: 6, fontSize: 9, color: 'var(--text-muted)' }}>{f.impact}</span>
              )}
            </div>
          </div>
        ))}

        {/* stats row */}
        <div style={{ display: 'flex', gap: 12, marginTop: 10, paddingTop: 8, borderTop: '1px solid rgba(67,70,85,0.15)' }}>
          <Stat label="MTTR" value={`~${a.mttr_estimate_mins}min`} />
          <Stat label="Cascade P" value={`${Math.round(a.cascade_probability * 100)}%`} />
          <Stat label="Events" value={a.event_count} />
          <Stat label="Cluster" value={r.cluster} />
        </div>

        {/* Raw JSON toggle */}
        <button
          onClick={() => setShowRaw(v => !v)}
          style={{
            marginTop: 10, display: 'flex', alignItems: 'center', gap: 4,
            background: 'transparent', border: 'none', color: 'var(--text-muted)',
            fontSize: 10, cursor: 'pointer', fontFamily: 'var(--font-mono)', padding: 0,
          }}
        >
          <Code2 size={9} />
          {showRaw ? 'hide' : 'view'} raw JSON
          <ChevronDown size={9} style={{ transform: showRaw ? 'rotate(180deg)' : 'none', transition: '0.2s' }} />
        </button>

        {showRaw && (
          <pre style={{
            marginTop: 8, fontFamily: 'var(--font-mono)', fontSize: 9.5,
            color: 'var(--text-secondary)', background: 'var(--surface-lowest)',
            borderRadius: 6, padding: 12, maxHeight: 160,
            overflow: 'auto', lineHeight: 1.6, whiteSpace: 'pre-wrap', wordBreak: 'break-all',
          }}>
            {msg.rawJson}
          </pre>
        )}
      </div>
    </div>
  );
}

function Stat({ label, value }) {
  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 1 }}>
      <span style={{ fontSize: 8, fontWeight: 700, textTransform: 'uppercase', letterSpacing: '0.8px', color: 'var(--text-dim)' }}>{label}</span>
      <span className="mono" style={{ fontSize: 10, color: 'var(--text-secondary)', fontWeight: 600 }}>{value}</span>
    </div>
  );
}

function ThinkingIndicator() {
  return (
    <div className="chat-thinking">
      <Cpu size={11} style={{ color: 'var(--primary)' }} />
      <span style={{ fontSize: 12 }}>Analyzing with MCP tool</span>
      <span className="thinking-dots">
        <span>.</span><span>.</span><span>.</span>
      </span>
    </div>
  );
}
