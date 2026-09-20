import { useState, useEffect } from 'react';
import { useParams, useNavigate } from 'react-router-dom';
import { getLead, initiateCall } from '../services/api';
import {
  ArrowLeft, Phone, Loader2, Building2, Mail, User,
  Briefcase, Target, AlertTriangle, DollarSign, Clock,
  Award, Lightbulb, FileText, CheckCircle, XCircle,
  Zap, MessageSquare, TrendingUp, Users as UsersIcon
} from 'lucide-react';

const interestColors = {
  High: 'bg-emerald-100 text-emerald-700 border-emerald-200',
  Medium: 'bg-amber-100 text-amber-700 border-amber-200',
  Low: 'bg-red-100 text-red-700 border-red-200',
  Unknown: 'bg-slate-100 text-slate-600 border-slate-200',
};

export default function LeadDetails() {
  const { id } = useParams();
  const navigate = useNavigate();
  const [lead, setLead] = useState(null);
  const [loading, setLoading] = useState(true);
  const [calling, setCalling] = useState(false);
  const [callMessage, setCallMessage] = useState('');

  const fetchLead = () => {
    getLead(id)
      .then(setLead)
      .catch(() => navigate('/leads'))
      .finally(() => setLoading(false));
  };

  useEffect(() => {
    fetchLead();
    // Poll for updates if call is in progress
    const interval = setInterval(() => {
      getLead(id).then(data => {
        setLead(data);
        if (data.call_status === 'Completed' && calling) {
          setCalling(false);
          setCallMessage('');
        }
      }).catch(() => {});
    }, 5000);
    return () => clearInterval(interval);
  }, [id]);

  const handleCall = async () => {
    if (!lead.phone) {
      alert('This lead does not have a phone number.');
      return;
    }
    setCalling(true);
    setCallMessage('Connecting to NOVA...');
    try {
      await initiateCall(id);
      setCallMessage('Call initiated ✓');
      fetchLead();
      setTimeout(() => {
        if (lead.call_status !== 'Completed') {
          setCallMessage('Call completed — processing results...');
        }
      }, 3000);
    } catch (err) {
      setCallMessage('');
      setCalling(false);
      const errorMsg = err.response?.data?.details || err.response?.data?.error || err.message;
      alert(`Unable to start the call: ${errorMsg}`);
    }
  };

  if (loading) return <DetailsSkeleton />;
  if (!lead) return null;

  const infoItems = [
    { icon: Building2, label: 'Company', value: lead.company },
    { icon: User, label: 'Job Title', value: lead.job_title },
    { icon: Phone, label: 'Phone', value: lead.phone },
    { icon: Mail, label: 'Email', value: lead.email },
    { icon: Briefcase, label: 'Industry', value: lead.industry },
    { icon: Target, label: 'Requirement', value: lead.customer_requirement },
    { icon: AlertTriangle, label: 'Pain Point', value: lead.pain_point },
    { icon: DollarSign, label: 'Budget', value: lead.budget },
    { icon: Clock, label: 'Timeline', value: lead.timeline },
    { icon: FileText, label: 'Notes', value: lead.notes },
  ];

  const novaItems = [
    { icon: Lightbulb, label: 'Current Process', value: lead.current_process },
    { icon: TrendingUp, label: 'Business Impact', value: lead.business_impact },
    { icon: Target, label: 'Desired Outcome', value: lead.desired_outcome },
    { icon: Award, label: 'Buying Criteria', value: lead.buying_criteria },
    { icon: UsersIcon, label: 'Decision Makers', value: lead.decision_makers },
    { icon: MessageSquare, label: 'Call Summary', value: lead.call_summary },
    { icon: CheckCircle, label: 'Recommended Action', value: lead.recommended_next_action },
  ];

  return (
    <div className="animate-fade-in">
      {/* Header */}
      <div className="flex items-start justify-between mb-8">
        <div className="flex items-center gap-4">
          <button onClick={() => navigate('/leads')} className="p-2 rounded-xl hover:bg-slate-100 text-slate-400 hover:text-slate-700">
            <ArrowLeft className="w-5 h-5" />
          </button>
          <div className="flex items-center gap-4">
            <div className="w-14 h-14 rounded-2xl bg-gradient-to-br from-nova-500 to-nova-700 flex items-center justify-center shadow-lg shadow-nova-500/25">
              <span className="text-xl font-bold text-white">
                {lead.lead_name?.split(' ').map(n => n[0]).join('').slice(0, 2)}
              </span>
            </div>
            <div>
              <h1 className="text-2xl font-bold text-slate-900">{lead.lead_name}</h1>
              <p className="text-slate-500">{lead.company || 'No company'} {lead.job_title ? `· ${lead.job_title}` : ''}</p>
            </div>
          </div>
        </div>

        {/* Call button */}
        <button
          onClick={handleCall}
          disabled={calling}
          className={`px-8 py-3.5 rounded-2xl text-white font-semibold text-base shadow-xl transition-all flex items-center gap-3
            ${calling
              ? 'bg-gradient-to-r from-amber-500 to-orange-500 shadow-amber-500/30 animate-pulse-ring'
              : 'bg-gradient-to-r from-nova-500 to-nova-700 shadow-nova-500/30 hover:shadow-nova-500/50 hover:-translate-y-0.5'
            }`}
        >
          {calling ? (
            <>
              <Loader2 className="w-5 h-5 animate-spin" />
              {callMessage || 'Calling...'}
            </>
          ) : (
            <>
              <Zap className="w-5 h-5" />
              CALL WITH NOVA
            </>
          )}
        </button>
      </div>

      {/* Status bar */}
      <div className="flex items-center gap-4 mb-6">
        <span className={`px-3 py-1.5 rounded-xl text-sm font-semibold border ${interestColors[lead.interest_level] || interestColors.Unknown}`}>
          {lead.interest_level || 'Unknown'} Interest
        </span>
        <div className="flex items-center gap-2 bg-white rounded-xl px-4 py-2 border border-border">
          <span className="text-sm text-slate-500">Lead Score</span>
          <span className="text-lg font-bold text-slate-900">{lead.lead_score || 0}</span>
          <div className="w-16 h-2 bg-slate-100 rounded-full overflow-hidden">
            <div
              className={`h-full rounded-full ${
                lead.lead_score >= 80 ? 'bg-emerald-500' :
                lead.lead_score >= 60 ? 'bg-amber-500' :
                lead.lead_score >= 40 ? 'bg-orange-400' : 'bg-slate-300'
              }`}
              style={{ width: `${lead.lead_score || 0}%` }}
            />
          </div>
        </div>
        <div className={`px-3 py-1.5 rounded-xl text-sm font-medium ${
          lead.call_status === 'Completed' ? 'bg-emerald-100 text-emerald-700' :
          lead.call_status === 'Calling' ? 'bg-blue-100 text-blue-700' :
          lead.call_status === 'Failed' ? 'bg-red-100 text-red-700' :
          'bg-slate-100 text-slate-600'
        }`}>
          {lead.call_status === 'Completed' ? <CheckCircle className="w-4 h-4 inline mr-1" /> :
           lead.call_status === 'Failed' ? <XCircle className="w-4 h-4 inline mr-1" /> : null}
          {lead.call_status || 'Not Called'}
        </div>
      </div>

      <div className="grid grid-cols-1 lg:grid-cols-2 gap-6">
        {/* Lead Information */}
        <div className="bg-white rounded-2xl border border-border p-6">
          <h2 className="text-lg font-semibold text-slate-900 mb-5 flex items-center gap-2">
            <User className="w-5 h-5 text-nova-500" />
            Lead Information
          </h2>
          <div className="space-y-4">
            {infoItems.map(({ icon: Icon, label, value }) => (
              <div key={label} className="flex items-start gap-3">
                <Icon className="w-4 h-4 text-slate-400 mt-0.5 shrink-0" />
                <div>
                  <p className="text-xs font-medium text-slate-400 uppercase tracking-wider">{label}</p>
                  <p className="text-sm text-slate-800 mt-0.5">{value || '—'}</p>
                </div>
              </div>
            ))}
          </div>
        </div>

        {/* NOVA Insights */}
        <div className="bg-white rounded-2xl border border-border p-6">
          <h2 className="text-lg font-semibold text-slate-900 mb-5 flex items-center gap-2">
            <Zap className="w-5 h-5 text-nova-500" />
            NOVA Insights
          </h2>
          {lead.call_status === 'Not Called' ? (
            <div className="text-center py-12">
              <div className="w-16 h-16 rounded-2xl bg-nova-50 flex items-center justify-center mx-auto mb-4">
                <Phone className="w-7 h-7 text-nova-400" />
              </div>
              <p className="text-slate-500 font-medium">No NOVA data yet</p>
              <p className="text-sm text-slate-400 mt-1">Click "Call with NOVA" to qualify this lead</p>
            </div>
          ) : (
            <div className="space-y-4">
              {novaItems.map(({ icon: Icon, label, value }) => (
                <div key={label} className="flex items-start gap-3">
                  <Icon className="w-4 h-4 text-nova-400 mt-0.5 shrink-0" />
                  <div>
                    <p className="text-xs font-medium text-slate-400 uppercase tracking-wider">{label}</p>
                    <p className="text-sm text-slate-800 mt-0.5">{value || '—'}</p>
                  </div>
                </div>
              ))}
            </div>
          )}
        </div>
      </div>

      {/* Transcript */}
      {lead.transcript && (
        <div className="mt-6 bg-white rounded-2xl border border-border p-6">
          <h2 className="text-lg font-semibold text-slate-900 mb-4 flex items-center gap-2">
            <MessageSquare className="w-5 h-5 text-nova-500" />
            Call Transcript
          </h2>
          <div className="bg-surface-secondary rounded-xl p-4 text-sm text-slate-700 leading-relaxed whitespace-pre-wrap max-h-96 overflow-y-auto">
            {lead.transcript}
          </div>
        </div>
      )}

      {/* Call History */}
      {lead.calls?.length > 0 && (
        <div className="mt-6 bg-white rounded-2xl border border-border p-6">
          <h2 className="text-lg font-semibold text-slate-900 mb-4 flex items-center gap-2">
            <Phone className="w-5 h-5 text-nova-500" />
            Call History
          </h2>
          <div className="space-y-3">
            {lead.calls.map((call) => (
              <div key={call.id} className="flex items-center justify-between p-3 rounded-xl bg-surface-secondary">
                <div className="flex items-center gap-3">
                  <div className={`w-8 h-8 rounded-lg flex items-center justify-center ${
                    call.status === 'completed' ? 'bg-emerald-100' : 
                    call.status === 'failed' ? 'bg-red-100' : 'bg-blue-100'
                  }`}>
                    <Phone className={`w-4 h-4 ${
                      call.status === 'completed' ? 'text-emerald-600' :
                      call.status === 'failed' ? 'text-red-600' : 'text-blue-600'
                    }`} />
                  </div>
                  <div>
                    <p className="text-sm font-medium text-slate-700 capitalize">{call.status}</p>
                    <p className="text-xs text-slate-400">{new Date(call.created_at).toLocaleString()}</p>
                  </div>
                </div>
                <div className="text-right">
                  <p className="text-sm text-slate-600">
                    {call.duration ? `${Math.floor(call.duration / 60)}m ${call.duration % 60}s` : '—'}
                  </p>
                  {call.lead_score && (
                    <p className="text-xs text-slate-400">Score: {call.lead_score}</p>
                  )}
                </div>
              </div>
            ))}
          </div>
        </div>
      )}
    </div>
  );
}

function DetailsSkeleton() {
  return (
    <div>
      <div className="flex items-center gap-4 mb-8">
        <div className="skeleton w-14 h-14 rounded-2xl" />
        <div>
          <div className="skeleton h-7 w-48 mb-2" />
          <div className="skeleton h-4 w-32" />
        </div>
      </div>
      <div className="grid grid-cols-1 lg:grid-cols-2 gap-6">
        <div className="bg-white rounded-2xl border border-border p-6">
          {[1,2,3,4,5].map(i => <div key={i} className="skeleton h-10 w-full mb-4" />)}
        </div>
        <div className="bg-white rounded-2xl border border-border p-6">
          {[1,2,3,4,5].map(i => <div key={i} className="skeleton h-10 w-full mb-4" />)}
        </div>
      </div>
    </div>
  );
}
