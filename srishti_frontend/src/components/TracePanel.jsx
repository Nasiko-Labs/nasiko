import React, { useRef, useEffect } from 'react';
import { Activity } from 'lucide-react';
import { toTimestamp } from '../simulation/mockData.js';

export default function TracePanel({ events, traceId }) {
  const listRef = useRef(null);

  useEffect(() => {
    if (listRef.current) {
      listRef.current.scrollTop = listRef.current.scrollHeight;
    }
  }, [events]);

  return (
    <div className="card" style={{ display: 'flex', flexDirection: 'column', height: '100%', padding: 20 }}>
      <div className="card-header">
        <span className="card-title"><Activity size={12} /> Live Traces</span>
        {events.length > 0 && (
          <span className="badge badge-blue">{events.length} events</span>
        )}
      </div>

      {events.length === 0 ? (
        <div style={{
          flex: 1,
          display: 'flex',
          flexDirection: 'column',
          alignItems: 'center',
          justifyContent: 'center',
          gap: 10,
          color: 'var(--text-dim)',
        }}>
          <Activity size={20} style={{ opacity: 0.3 }} />
          <span style={{ fontSize: 11, textAlign: 'center' }}>Invoke a tool to see live traces</span>
        </div>
      ) : (
        <>
          <div className="trace-list" ref={listRef}>
            {events.map((ev, i) => (
              <div key={i} className="trace-event">
                <span className={`trace-dot ${ev.color}`} />
                <div className="trace-body">
                  <div className="trace-time">
                    {toTimestamp(ev.baseTime, ev.ms)}
                  </div>
                  <div className="trace-msg">
                    <strong>{ev.event}</strong>
                    {ev.latency && (
                      <span className={`trace-latency ${ev.color === 'green' ? 'green-l' : 'amber-l'}`} style={{ marginLeft: 6 }}>
                        {ev.latency}
                      </span>
                    )}
                  </div>
                </div>
              </div>
            ))}
          </div>

          {traceId && (
            <div className="trace-footer" style={{ borderTop: '1px solid rgba(67,70,85,0.15)', paddingTop: 12, marginTop: 12 }}>
              <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center', marginBottom: 8 }}>
                <span className="badge badge-gray mono" style={{ fontSize: 9, padding: '2px 6px' }}>W3C Trace Context</span>
                <span style={{ fontSize: 10, color: 'var(--success)', display: 'flex', alignItems: 'center', gap: 4 }}>
                  <span style={{ width: 6, height: 6, background: 'var(--success)', borderRadius: '50%' }} />
                  Exporting to Arize Phoenix
                </span>
              </div>
              <div className="trace-id-row">
                <span className="mono">TRACE ID</span>
                <span className="trace-id-val mono">{traceId}</span>
              </div>
              <div style={{ display: 'flex', gap: 12, marginTop: 6 }}>
                 <span style={{ fontSize: 9, color: 'var(--text-dim)', fontFamily: 'var(--font-mono)' }}>OTLP/http</span>
                 <span style={{ fontSize: 9, color: 'var(--text-dim)', fontFamily: 'var(--font-mono)' }}>spans: {events.length}</span>
              </div>
            </div>
          )}
        </>
      )}
    </div>
  );
}
