import { useState, useEffect } from 'react';
import { Link } from 'react-router-dom';
import { getDashboard } from '../services/api';
import { Users, PhoneCall, Star, TrendingUp, ArrowRight, Clock, Building2 } from 'lucide-react';

const statCards = [
  { key: 'totalLeads', label: 'Total Leads', icon: Users, color: 'from-nova-500 to-nova-600', shadow: 'shadow-nova-500/20' },
  { key: 'callsCompleted', label: 'Calls Completed', icon: PhoneCall, color: 'from-accent-green to-emerald-600', shadow: 'shadow-emerald-500/20' },
  { key: 'highInterest', label: 'High Interest', icon: Star, color: 'from-accent-amber to-orange-500', shadow: 'shadow-amber-500/20' },
  { key: 'avgScore', label: 'Avg Lead Score', icon: TrendingUp, color: 'from-accent-blue to-blue-600', shadow: 'shadow-blue-500/20' },
];

const interestColors = {
  High: 'bg-emerald-100 text-emerald-700',
  Medium: 'bg-amber-100 text-amber-700',
  Low: 'bg-red-100 text-red-700',
  Unknown: 'bg-slate-100 text-slate-600',
};

const statusColors = {
  Completed: 'bg-emerald-100 text-emerald-700',
  Calling: 'bg-blue-100 text-blue-700',
  'Not Called': 'bg-slate-100 text-slate-600',
  Failed: 'bg-red-100 text-red-700',
};

export default function Dashboard() {
  const [data, setData] = useState(null);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    getDashboard()
      .then(setData)
      .catch(console.error)
      .finally(() => setLoading(false));
  }, []);

  if (loading) return <DashboardSkeleton />;

  return (
    <div className="animate-fade-in">
      {/* Header */}
      <div className="mb-8">
        <h1 className="text-2xl font-bold text-slate-900">Dashboard</h1>
        <p className="text-slate-500 mt-1">Overview of your sales pipeline</p>
      </div>

      {/* KPI Cards */}
      <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-4 gap-5 mb-8">
        {statCards.map(({ key, label, icon: Icon, color, shadow }, i) => (
          <div
            key={key}
            className="bg-white rounded-2xl p-5 border border-border hover:shadow-lg hover:-translate-y-0.5 transition-all duration-300"
            style={{ animationDelay: `${i * 80}ms` }}
          >
            <div className="flex items-start justify-between">
              <div>
                <p className="text-sm font-medium text-slate-500">{label}</p>
                <p className="text-3xl font-bold text-slate-900 mt-1">
                  {data?.stats?.[key] ?? 0}
                </p>
              </div>
              <div className={`w-11 h-11 rounded-xl bg-gradient-to-br ${color} ${shadow} shadow-lg flex items-center justify-center`}>
                <Icon className="w-5 h-5 text-white" />
              </div>
            </div>
          </div>
        ))}
      </div>

      <div className="grid grid-cols-1 lg:grid-cols-3 gap-6">
        {/* Recent Leads - takes 2 cols */}
        <div className="lg:col-span-2 bg-white rounded-2xl border border-border p-6">
          <div className="flex items-center justify-between mb-5">
            <h2 className="text-lg font-semibold text-slate-900">Recent Leads</h2>
            <Link to="/leads" className="text-sm text-nova-600 hover:text-nova-700 font-medium flex items-center gap-1">
              View all <ArrowRight className="w-4 h-4" />
            </Link>
          </div>
          <div className="space-y-3">
            {data?.recentLeads?.map((lead) => (
              <Link
                to={`/leads/${lead.id}`}
                key={lead.id}
                className="flex items-center justify-between p-3 rounded-xl hover:bg-slate-50 transition-colors group"
              >
                <div className="flex items-center gap-3">
                  <div className="w-10 h-10 rounded-xl bg-gradient-to-br from-nova-100 to-nova-200 flex items-center justify-center">
                    <span className="text-sm font-bold text-nova-700">
                      {lead.lead_name?.split(' ').map(n => n[0]).join('').slice(0, 2)}
                    </span>
                  </div>
                  <div>
                    <p className="text-sm font-semibold text-slate-800 group-hover:text-nova-700">{lead.lead_name}</p>
                    <div className="flex items-center gap-1 text-xs text-slate-400">
                      <Building2 className="w-3 h-3" />
                      {lead.company || 'No company'}
                    </div>
                  </div>
                </div>
                <div className="flex items-center gap-3">
                  <span className={`px-2.5 py-1 rounded-lg text-xs font-medium ${interestColors[lead.interest_level] || interestColors.Unknown}`}>
                    {lead.interest_level || 'Unknown'}
                  </span>
                  <div className="text-right">
                    <p className="text-sm font-bold text-slate-700">{lead.lead_score || 0}</p>
                    <p className="text-[10px] text-slate-400">Score</p>
                  </div>
                </div>
              </Link>
            ))}
            {(!data?.recentLeads || data.recentLeads.length === 0) && (
              <p className="text-sm text-slate-400 text-center py-8">No leads yet. Add your first lead!</p>
            )}
          </div>
        </div>

        {/* Interest Distribution */}
        <div className="bg-white rounded-2xl border border-border p-6">
          <h2 className="text-lg font-semibold text-slate-900 mb-5">Lead Interest</h2>
          <div className="space-y-4">
            {data?.interestDistribution?.map(({ interest_level, count }) => {
              const total = data.stats.totalLeads || 1;
              const pct = Math.round((count / total) * 100);
              const barColor = {
                High: 'bg-emerald-500',
                Medium: 'bg-amber-500',
                Low: 'bg-red-400',
                Unknown: 'bg-slate-300'
              }[interest_level] || 'bg-slate-300';

              return (
                <div key={interest_level}>
                  <div className="flex justify-between text-sm mb-1.5">
                    <span className="font-medium text-slate-700">{interest_level}</span>
                    <span className="text-slate-400">{count} ({pct}%)</span>
                  </div>
                  <div className="h-2 bg-slate-100 rounded-full overflow-hidden">
                    <div className={`h-full ${barColor} rounded-full transition-all duration-700`} style={{ width: `${pct}%` }} />
                  </div>
                </div>
              );
            })}
          </div>

          <hr className="my-5 border-border" />

          <h3 className="text-sm font-semibold text-slate-700 mb-3">Call Status</h3>
          <div className="space-y-2">
            {data?.callStatusDistribution?.map(({ call_status, count }) => (
              <div key={call_status} className="flex items-center justify-between text-sm">
                <span className={`px-2 py-0.5 rounded-md text-xs font-medium ${statusColors[call_status] || statusColors['Not Called']}`}>
                  {call_status}
                </span>
                <span className="font-medium text-slate-600">{count}</span>
              </div>
            ))}
          </div>
        </div>
      </div>

      {/* Recent Calls */}
      <div className="mt-6 bg-white rounded-2xl border border-border p-6">
        <div className="flex items-center justify-between mb-5">
          <h2 className="text-lg font-semibold text-slate-900">Recent Calls</h2>
          <Link to="/calls" className="text-sm text-nova-600 hover:text-nova-700 font-medium flex items-center gap-1">
            View all <ArrowRight className="w-4 h-4" />
          </Link>
        </div>
        {data?.recentCalls?.length > 0 ? (
          <div className="overflow-x-auto">
            <table className="w-full">
              <thead>
                <tr className="border-b border-border">
                  <th className="text-left text-xs font-semibold text-slate-500 uppercase tracking-wider pb-3 px-3">Lead</th>
                  <th className="text-left text-xs font-semibold text-slate-500 uppercase tracking-wider pb-3 px-3">Phone</th>
                  <th className="text-left text-xs font-semibold text-slate-500 uppercase tracking-wider pb-3 px-3">Status</th>
                  <th className="text-left text-xs font-semibold text-slate-500 uppercase tracking-wider pb-3 px-3">Duration</th>
                  <th className="text-left text-xs font-semibold text-slate-500 uppercase tracking-wider pb-3 px-3">Date</th>
                </tr>
              </thead>
              <tbody>
                {data.recentCalls.map((call) => (
                  <tr key={call.id} className="border-b border-border/50 hover:bg-slate-50">
                    <td className="py-3 px-3">
                      <p className="text-sm font-medium text-slate-800">{call.lead_name || 'Unknown'}</p>
                      <p className="text-xs text-slate-400">{call.company || ''}</p>
                    </td>
                    <td className="py-3 px-3 text-sm text-slate-600">{call.phone}</td>
                    <td className="py-3 px-3">
                      <span className={`px-2 py-0.5 rounded-md text-xs font-medium ${
                        call.status === 'completed' ? 'bg-emerald-100 text-emerald-700' :
                        call.status === 'failed' ? 'bg-red-100 text-red-700' :
                        'bg-blue-100 text-blue-700'
                      }`}>{call.status}</span>
                    </td>
                    <td className="py-3 px-3 text-sm text-slate-600">
                      <div className="flex items-center gap-1">
                        <Clock className="w-3.5 h-3.5 text-slate-400" />
                        {call.duration ? `${Math.floor(call.duration / 60)}m ${call.duration % 60}s` : '-'}
                      </div>
                    </td>
                    <td className="py-3 px-3 text-sm text-slate-500">
                      {new Date(call.created_at).toLocaleDateString()}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        ) : (
          <p className="text-sm text-slate-400 text-center py-8">No calls yet</p>
        )}
      </div>
    </div>
  );
}

function DashboardSkeleton() {
  return (
    <div>
      <div className="mb-8">
        <div className="skeleton h-7 w-40 mb-2" />
        <div className="skeleton h-4 w-64" />
      </div>
      <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-4 gap-5 mb-8">
        {[1,2,3,4].map(i => (
          <div key={i} className="bg-white rounded-2xl p-5 border border-border">
            <div className="skeleton h-4 w-24 mb-2" />
            <div className="skeleton h-8 w-16" />
          </div>
        ))}
      </div>
      <div className="grid grid-cols-1 lg:grid-cols-3 gap-6">
        <div className="lg:col-span-2 bg-white rounded-2xl border border-border p-6">
          <div className="skeleton h-6 w-32 mb-4" />
          {[1,2,3].map(i => <div key={i} className="skeleton h-14 w-full mb-3" />)}
        </div>
        <div className="bg-white rounded-2xl border border-border p-6">
          <div className="skeleton h-6 w-32 mb-4" />
          {[1,2,3].map(i => <div key={i} className="skeleton h-8 w-full mb-3" />)}
        </div>
      </div>
    </div>
  );
}
