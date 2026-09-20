import { useState, useEffect } from 'react';
import { Link, useNavigate } from 'react-router-dom';
import { getLeads, initiateCall } from '../services/api';
import { Search, Filter, Phone, Eye, ArrowUpDown, Loader2, Building2, Mail } from 'lucide-react';

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

export default function Leads() {
  const [leads, setLeads] = useState([]);
  const [loading, setLoading] = useState(true);
  const [search, setSearch] = useState('');
  const [interestFilter, setInterestFilter] = useState('All');
  const [statusFilter, setStatusFilter] = useState('All');
  const [sortBy, setSortBy] = useState('created_at');
  const [sortOrder, setSortOrder] = useState('DESC');
  const [callingId, setCallingId] = useState(null);
  const navigate = useNavigate();

  const fetchLeads = () => {
    const params = {};
    if (search) params.search = search;
    if (interestFilter !== 'All') params.interest_level = interestFilter;
    if (statusFilter !== 'All') params.call_status = statusFilter;
    params.sort_by = sortBy;
    params.order = sortOrder;

    getLeads(params)
      .then(setLeads)
      .catch(console.error)
      .finally(() => setLoading(false));
  };

  useEffect(() => {
    const timer = setTimeout(fetchLeads, 300);
    return () => clearTimeout(timer);
  }, [search, interestFilter, statusFilter, sortBy, sortOrder]);

  const handleCall = async (e, leadId) => {
    e.stopPropagation();
    setCallingId(leadId);
    try {
      await initiateCall(leadId);
      fetchLeads();
    } catch (err) {
      const errorMsg = err.response?.data?.details || err.response?.data?.error || err.message;
      alert(`Unable to start the call: ${errorMsg}`);
    } finally {
      setTimeout(() => setCallingId(null), 2000);
    }
  };

  const toggleSort = (col) => {
    if (sortBy === col) {
      setSortOrder(sortOrder === 'ASC' ? 'DESC' : 'ASC');
    } else {
      setSortBy(col);
      setSortOrder('DESC');
    }
  };

  return (
    <div className="animate-fade-in">
      {/* Header */}
      <div className="flex items-center justify-between mb-6">
        <div>
          <h1 className="text-2xl font-bold text-slate-900">Leads</h1>
          <p className="text-slate-500 mt-1">{leads.length} total leads in your pipeline</p>
        </div>
        <Link
          to="/leads/new"
          className="px-5 py-2.5 bg-gradient-to-r from-nova-500 to-nova-600 text-white text-sm font-semibold rounded-xl shadow-lg shadow-nova-500/25 hover:shadow-nova-500/40 hover:-translate-y-0.5 transition-all"
        >
          + Add Lead
        </Link>
      </div>

      {/* Filters */}
      <div className="bg-white rounded-2xl border border-border p-4 mb-6">
        <div className="flex flex-wrap items-center gap-3">
          {/* Search */}
          <div className="relative flex-1 min-w-[220px]">
            <Search className="absolute left-3 top-1/2 -translate-y-1/2 w-4 h-4 text-slate-400" />
            <input
              type="text"
              placeholder="Search leads..."
              value={search}
              onChange={(e) => setSearch(e.target.value)}
              className="w-full pl-10 pr-4 py-2.5 text-sm border border-border rounded-xl focus:outline-none focus:ring-2 focus:ring-nova-500/20 focus:border-nova-400 bg-surface-secondary"
            />
          </div>

          {/* Interest filter */}
          <div className="flex items-center gap-2">
            <Filter className="w-4 h-4 text-slate-400" />
            <select
              value={interestFilter}
              onChange={(e) => setInterestFilter(e.target.value)}
              className="text-sm border border-border rounded-xl px-3 py-2.5 focus:outline-none focus:ring-2 focus:ring-nova-500/20 bg-surface-secondary"
            >
              <option value="All">All Interest</option>
              <option value="High">High</option>
              <option value="Medium">Medium</option>
              <option value="Low">Low</option>
              <option value="Unknown">Unknown</option>
            </select>
          </div>

          {/* Status filter */}
          <select
            value={statusFilter}
            onChange={(e) => setStatusFilter(e.target.value)}
            className="text-sm border border-border rounded-xl px-3 py-2.5 focus:outline-none focus:ring-2 focus:ring-nova-500/20 bg-surface-secondary"
          >
            <option value="All">All Status</option>
            <option value="Not Called">Not Called</option>
            <option value="Calling">Calling</option>
            <option value="Completed">Completed</option>
            <option value="Failed">Failed</option>
          </select>
        </div>
      </div>

      {/* Table */}
      <div className="bg-white rounded-2xl border border-border overflow-hidden">
        {loading ? (
          <div className="p-8">
            {[1,2,3,4,5].map(i => <div key={i} className="skeleton h-16 w-full mb-3" />)}
          </div>
        ) : leads.length === 0 ? (
          <div className="text-center py-16">
            <div className="w-16 h-16 rounded-2xl bg-nova-50 flex items-center justify-center mx-auto mb-4">
              <Search className="w-7 h-7 text-nova-400" />
            </div>
            <p className="text-lg font-semibold text-slate-700">No leads found</p>
            <p className="text-sm text-slate-400 mt-1">Try adjusting your filters or add a new lead</p>
          </div>
        ) : (
          <div className="overflow-x-auto">
            <table className="w-full">
              <thead>
                <tr className="bg-surface-secondary border-b border-border">
                  <th className="text-left text-xs font-semibold text-slate-500 uppercase tracking-wider py-3 px-4">Lead</th>
                  <th className="text-left text-xs font-semibold text-slate-500 uppercase tracking-wider py-3 px-4">Contact</th>
                  <th className="text-left text-xs font-semibold text-slate-500 uppercase tracking-wider py-3 px-4">Requirement</th>
                  <th className="text-left text-xs font-semibold text-slate-500 uppercase tracking-wider py-3 px-4">Budget</th>
                  <th className="text-left text-xs font-semibold text-slate-500 uppercase tracking-wider py-3 px-4">
                    <button onClick={() => toggleSort('interest_level')} className="flex items-center gap-1 hover:text-slate-700">
                      Interest <ArrowUpDown className="w-3 h-3" />
                    </button>
                  </th>
                  <th className="text-left text-xs font-semibold text-slate-500 uppercase tracking-wider py-3 px-4">
                    <button onClick={() => toggleSort('lead_score')} className="flex items-center gap-1 hover:text-slate-700">
                      Score <ArrowUpDown className="w-3 h-3" />
                    </button>
                  </th>
                  <th className="text-left text-xs font-semibold text-slate-500 uppercase tracking-wider py-3 px-4">Status</th>
                  <th className="text-left text-xs font-semibold text-slate-500 uppercase tracking-wider py-3 px-4">Next Action</th>
                  <th className="text-right text-xs font-semibold text-slate-500 uppercase tracking-wider py-3 px-4">Actions</th>
                </tr>
              </thead>
              <tbody>
                {leads.map((lead, idx) => (
                  <tr
                    key={lead.id}
                    className="border-b border-border/50 hover:bg-nova-50/30 cursor-pointer transition-colors"
                    onClick={() => navigate(`/leads/${lead.id}`)}
                    style={{ animationDelay: `${idx * 40}ms` }}
                  >
                    <td className="py-3 px-4">
                      <div className="flex items-center gap-3">
                        <div className="w-9 h-9 rounded-lg bg-gradient-to-br from-nova-100 to-nova-200 flex items-center justify-center shrink-0">
                          <span className="text-xs font-bold text-nova-700">
                            {lead.lead_name?.split(' ').map(n => n[0]).join('').slice(0, 2)}
                          </span>
                        </div>
                        <div>
                          <p className="text-sm font-semibold text-slate-800">{lead.lead_name}</p>
                          <div className="flex items-center gap-1 text-xs text-slate-400">
                            <Building2 className="w-3 h-3" />
                            {lead.company || '—'}
                          </div>
                        </div>
                      </div>
                    </td>
                    <td className="py-3 px-4">
                      <p className="text-sm text-slate-700">{lead.phone || '—'}</p>
                      <div className="flex items-center gap-1 text-xs text-slate-400">
                        <Mail className="w-3 h-3" />
                        {lead.email || '—'}
                      </div>
                    </td>
                    <td className="py-3 px-4">
                      <p className="text-sm text-slate-600 max-w-[180px] truncate">{lead.customer_requirement || '—'}</p>
                    </td>
                    <td className="py-3 px-4 text-sm text-slate-600">{lead.budget || '—'}</td>
                    <td className="py-3 px-4">
                      <span className={`px-2.5 py-1 rounded-lg text-xs font-medium ${interestColors[lead.interest_level] || interestColors.Unknown}`}>
                        {lead.interest_level || 'Unknown'}
                      </span>
                    </td>
                    <td className="py-3 px-4">
                      <div className="flex items-center gap-2">
                        <div className="w-8 h-1.5 bg-slate-100 rounded-full overflow-hidden">
                          <div
                            className={`h-full rounded-full ${
                              lead.lead_score >= 80 ? 'bg-emerald-500' :
                              lead.lead_score >= 60 ? 'bg-amber-500' :
                              lead.lead_score >= 40 ? 'bg-orange-400' : 'bg-slate-300'
                            }`}
                            style={{ width: `${lead.lead_score || 0}%` }}
                          />
                        </div>
                        <span className="text-sm font-semibold text-slate-700">{lead.lead_score || 0}</span>
                      </div>
                    </td>
                    <td className="py-3 px-4">
                      <span className={`px-2.5 py-1 rounded-lg text-xs font-medium ${statusColors[lead.call_status] || statusColors['Not Called']}`}>
                        {lead.call_status || 'Not Called'}
                      </span>
                    </td>
                    <td className="py-3 px-4">
                      <p className="text-xs text-slate-500 max-w-[150px] truncate">{lead.recommended_next_action || '—'}</p>
                    </td>
                    <td className="py-3 px-4">
                      <div className="flex items-center justify-end gap-2">
                        <button
                          onClick={(e) => { e.stopPropagation(); navigate(`/leads/${lead.id}`); }}
                          className="p-2 rounded-lg text-slate-400 hover:text-nova-600 hover:bg-nova-50 transition-colors"
                          title="View Lead"
                        >
                          <Eye className="w-4 h-4" />
                        </button>
                        <button
                          onClick={(e) => handleCall(e, lead.id)}
                          disabled={callingId === lead.id || lead.call_status === 'Calling'}
                          className="p-2 rounded-lg text-slate-400 hover:text-white hover:bg-nova-500 transition-colors disabled:opacity-50"
                          title="Call Lead"
                        >
                          {callingId === lead.id ? (
                            <Loader2 className="w-4 h-4 animate-spin" />
                          ) : (
                            <Phone className="w-4 h-4" />
                          )}
                        </button>
                      </div>
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
