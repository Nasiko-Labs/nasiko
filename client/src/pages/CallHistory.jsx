import { useState, useEffect } from 'react';
import { getCalls } from '../services/api';
import { Link } from 'react-router-dom';
import { Phone, Clock, ArrowRight, MessageSquare, X } from 'lucide-react';

const statusColors = {
  completed: 'bg-emerald-100 text-emerald-700',
  failed: 'bg-red-100 text-red-700',
  initiated: 'bg-blue-100 text-blue-700',
};

export default function CallHistory() {
  const [calls, setCalls] = useState([]);
  const [loading, setLoading] = useState(true);
  const [selectedCall, setSelectedCall] = useState(null);

  useEffect(() => {
    getCalls()
      .then(setCalls)
      .catch(console.error)
      .finally(() => setLoading(false));
  }, []);

  return (
    <div className="animate-fade-in">
      {/* Header */}
      <div className="mb-6">
        <h1 className="text-2xl font-bold text-slate-900">Call History</h1>
        <p className="text-slate-500 mt-1">{calls.length} total calls recorded</p>
      </div>

      {/* Call detail modal */}
      {selectedCall && (
        <div className="fixed inset-0 z-50 bg-black/20 backdrop-blur-sm flex items-center justify-center p-4" onClick={() => setSelectedCall(null)}>
          <div className="bg-white rounded-2xl shadow-2xl max-w-lg w-full max-h-[80vh] overflow-y-auto p-6 animate-fade-in" onClick={e => e.stopPropagation()}>
            <div className="flex items-center justify-between mb-4">
              <h2 className="text-lg font-semibold text-slate-900">Call Details</h2>
              <button onClick={() => setSelectedCall(null)} className="p-1.5 rounded-lg hover:bg-slate-100">
                <X className="w-5 h-5 text-slate-400" />
              </button>
            </div>
            
            <div className="space-y-3 text-sm">
              <div className="flex justify-between py-2 border-b border-border/50">
                <span className="text-slate-400">Lead</span>
                <span className="font-medium text-slate-800">{selectedCall.lead_name || 'Unknown'}</span>
              </div>
              <div className="flex justify-between py-2 border-b border-border/50">
                <span className="text-slate-400">Company</span>
                <span className="font-medium text-slate-800">{selectedCall.company || '—'}</span>
              </div>
              <div className="flex justify-between py-2 border-b border-border/50">
                <span className="text-slate-400">Phone</span>
                <span className="font-medium text-slate-800">{selectedCall.phone || '—'}</span>
              </div>
              <div className="flex justify-between py-2 border-b border-border/50">
                <span className="text-slate-400">Status</span>
                <span className={`px-2 py-0.5 rounded-md text-xs font-medium capitalize ${statusColors[selectedCall.status] || 'bg-slate-100 text-slate-600'}`}>
                  {selectedCall.status}
                </span>
              </div>
              <div className="flex justify-between py-2 border-b border-border/50">
                <span className="text-slate-400">Duration</span>
                <span className="font-medium text-slate-800">
                  {selectedCall.duration ? `${Math.floor(selectedCall.duration / 60)}m ${selectedCall.duration % 60}s` : '—'}
                </span>
              </div>
              <div className="flex justify-between py-2 border-b border-border/50">
                <span className="text-slate-400">Interest Level</span>
                <span className="font-medium text-slate-800">{selectedCall.interest_level || '—'}</span>
              </div>
              <div className="flex justify-between py-2 border-b border-border/50">
                <span className="text-slate-400">Lead Score</span>
                <span className="font-medium text-slate-800">{selectedCall.lead_score || '—'}</span>
              </div>
              <div className="flex justify-between py-2 border-b border-border/50">
                <span className="text-slate-400">Date</span>
                <span className="font-medium text-slate-800">{new Date(selectedCall.created_at).toLocaleString()}</span>
              </div>
            </div>

            {selectedCall.summary && (
              <div className="mt-4">
                <p className="text-xs font-semibold text-slate-500 uppercase tracking-wider mb-2">Summary</p>
                <p className="text-sm text-slate-700 bg-surface-secondary rounded-xl p-3">{selectedCall.summary}</p>
              </div>
            )}

            {selectedCall.transcript && (
              <div className="mt-4">
                <p className="text-xs font-semibold text-slate-500 uppercase tracking-wider mb-2">Transcript</p>
                <div className="text-sm text-slate-700 bg-surface-secondary rounded-xl p-3 max-h-48 overflow-y-auto whitespace-pre-wrap">
                  {selectedCall.transcript}
                </div>
              </div>
            )}

            <div className="mt-5 flex justify-end">
              <Link
                to={`/leads/${selectedCall.lead_id}`}
                className="text-sm text-nova-600 hover:text-nova-700 font-medium flex items-center gap-1"
              >
                View Lead <ArrowRight className="w-4 h-4" />
              </Link>
            </div>
          </div>
        </div>
      )}

      {/* Table */}
      <div className="bg-white rounded-2xl border border-border overflow-hidden">
        {loading ? (
          <div className="p-8">
            {[1,2,3,4].map(i => <div key={i} className="skeleton h-16 w-full mb-3" />)}
          </div>
        ) : calls.length === 0 ? (
          <div className="text-center py-16">
            <div className="w-16 h-16 rounded-2xl bg-nova-50 flex items-center justify-center mx-auto mb-4">
              <Phone className="w-7 h-7 text-nova-400" />
            </div>
            <p className="text-lg font-semibold text-slate-700">No calls yet</p>
            <p className="text-sm text-slate-400 mt-1">Calls will appear here once you start calling leads with NOVA</p>
          </div>
        ) : (
          <div className="overflow-x-auto">
            <table className="w-full">
              <thead>
                <tr className="bg-surface-secondary border-b border-border">
                  <th className="text-left text-xs font-semibold text-slate-500 uppercase tracking-wider py-3 px-4">Lead</th>
                  <th className="text-left text-xs font-semibold text-slate-500 uppercase tracking-wider py-3 px-4">Phone</th>
                  <th className="text-left text-xs font-semibold text-slate-500 uppercase tracking-wider py-3 px-4">Date</th>
                  <th className="text-left text-xs font-semibold text-slate-500 uppercase tracking-wider py-3 px-4">Duration</th>
                  <th className="text-left text-xs font-semibold text-slate-500 uppercase tracking-wider py-3 px-4">Status</th>
                  <th className="text-left text-xs font-semibold text-slate-500 uppercase tracking-wider py-3 px-4">Interest</th>
                  <th className="text-left text-xs font-semibold text-slate-500 uppercase tracking-wider py-3 px-4">Score</th>
                  <th className="text-left text-xs font-semibold text-slate-500 uppercase tracking-wider py-3 px-4">Summary</th>
                  <th className="text-right text-xs font-semibold text-slate-500 uppercase tracking-wider py-3 px-4"></th>
                </tr>
              </thead>
              <tbody>
                {calls.map((call, idx) => (
                  <tr
                    key={call.id}
                    className="border-b border-border/50 hover:bg-nova-50/30 cursor-pointer transition-colors"
                    onClick={() => setSelectedCall(call)}
                  >
                    <td className="py-3 px-4">
                      <p className="text-sm font-semibold text-slate-800">{call.lead_name || 'Unknown'}</p>
                      <p className="text-xs text-slate-400">{call.company || ''}</p>
                    </td>
                    <td className="py-3 px-4 text-sm text-slate-600">{call.phone || '—'}</td>
                    <td className="py-3 px-4 text-sm text-slate-500">{new Date(call.created_at).toLocaleDateString()}</td>
                    <td className="py-3 px-4">
                      <div className="flex items-center gap-1 text-sm text-slate-600">
                        <Clock className="w-3.5 h-3.5 text-slate-400" />
                        {call.duration ? `${Math.floor(call.duration / 60)}m ${call.duration % 60}s` : '—'}
                      </div>
                    </td>
                    <td className="py-3 px-4">
                      <span className={`px-2.5 py-1 rounded-lg text-xs font-medium capitalize ${statusColors[call.status] || 'bg-slate-100 text-slate-600'}`}>
                        {call.status}
                      </span>
                    </td>
                    <td className="py-3 px-4">
                      <span className="text-sm text-slate-600">{call.interest_level || '—'}</span>
                    </td>
                    <td className="py-3 px-4">
                      <span className="text-sm font-semibold text-slate-700">{call.lead_score || '—'}</span>
                    </td>
                    <td className="py-3 px-4">
                      <p className="text-xs text-slate-500 max-w-[200px] truncate">{call.summary || '—'}</p>
                    </td>
                    <td className="py-3 px-4 text-right">
                      <button className="p-1.5 rounded-lg text-slate-400 hover:text-nova-600 hover:bg-nova-50">
                        {call.transcript ? <MessageSquare className="w-4 h-4" /> : <ArrowRight className="w-4 h-4" />}
                      </button>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </div>
    </div>
  );
}
