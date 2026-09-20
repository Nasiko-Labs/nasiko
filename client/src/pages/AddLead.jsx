import { useState } from 'react';
import { useNavigate } from 'react-router-dom';
import { createLead } from '../services/api';
import { UserPlus, ArrowLeft, Loader2, CheckCircle } from 'lucide-react';

export default function AddLead() {
  const navigate = useNavigate();
  const [saving, setSaving] = useState(false);
  const [saved, setSaved] = useState(false);
  const [form, setForm] = useState({
    lead_name: '',
    company: '',
    phone: '',
    email: '',
    job_title: '',
    industry: '',
    customer_requirement: '',
    pain_point: '',
    budget: '',
    timeline: '',
    notes: '',
  });

  const handleChange = (e) => {
    setForm({ ...form, [e.target.name]: e.target.value });
  };

  const handleSubmit = async (e) => {
    e.preventDefault();
    if (!form.lead_name.trim()) {
      alert('Lead name is required');
      return;
    }

    setSaving(true);
    try {
      const lead = await createLead(form);
      setSaved(true);
      setTimeout(() => navigate(`/leads/${lead.id}`), 1200);
    } catch (err) {
      alert('Failed to save lead. Please try again.');
    } finally {
      setSaving(false);
    }
  };

  const fields = [
    { name: 'lead_name', label: 'Lead Name *', placeholder: 'e.g. Rahul Sharma', required: true },
    { name: 'company', label: 'Company', placeholder: 'e.g. ABC Technologies' },
    { name: 'phone', label: 'Phone', placeholder: 'e.g. +919876543210' },
    { name: 'email', label: 'Email', placeholder: 'e.g. rahul@company.com', type: 'email' },
    { name: 'job_title', label: 'Job Title', placeholder: 'e.g. Operations Manager' },
    { name: 'industry', label: 'Industry', placeholder: 'e.g. Information Technology' },
    { name: 'customer_requirement', label: 'Product/Service Interest', placeholder: 'What are they looking for?', full: true },
    { name: 'pain_point', label: 'Known Pain Point', placeholder: 'What problem are they trying to solve?', full: true },
    { name: 'budget', label: 'Budget', placeholder: 'e.g. ₹5,00,000' },
    { name: 'timeline', label: 'Timeline', placeholder: 'e.g. 1 month' },
    { name: 'notes', label: 'Notes', placeholder: 'Any additional context for NOVA...', full: true, textarea: true },
  ];

  return (
    <div className="animate-fade-in max-w-3xl mx-auto">
      {/* Header */}
      <div className="flex items-center gap-4 mb-8">
        <button onClick={() => navigate(-1)} className="p-2 rounded-xl hover:bg-slate-100 text-slate-400 hover:text-slate-700">
          <ArrowLeft className="w-5 h-5" />
        </button>
        <div>
          <h1 className="text-2xl font-bold text-slate-900">Add New Lead</h1>
          <p className="text-slate-500 mt-0.5">Enter lead information for NOVA qualification</p>
        </div>
      </div>

      {/* Success overlay */}
      {saved && (
        <div className="fixed inset-0 z-50 bg-black/20 backdrop-blur-sm flex items-center justify-center">
          <div className="bg-white rounded-2xl p-8 shadow-2xl text-center animate-fade-in">
            <CheckCircle className="w-16 h-16 text-emerald-500 mx-auto mb-4" />
            <h2 className="text-xl font-bold text-slate-900">Lead Saved!</h2>
            <p className="text-slate-500 mt-1">Redirecting to lead details...</p>
          </div>
        </div>
      )}

      {/* Form */}
      <form onSubmit={handleSubmit} className="bg-white rounded-2xl border border-border p-8">
        <div className="grid grid-cols-1 md:grid-cols-2 gap-5">
          {fields.map(({ name, label, placeholder, type, required, full, textarea }) => (
            <div key={name} className={full ? 'md:col-span-2' : ''}>
              <label className="block text-sm font-medium text-slate-700 mb-1.5">{label}</label>
              {textarea ? (
                <textarea
                  name={name}
                  value={form[name]}
                  onChange={handleChange}
                  placeholder={placeholder}
                  rows={3}
                  className="w-full px-4 py-2.5 text-sm border border-border rounded-xl focus:outline-none focus:ring-2 focus:ring-nova-500/20 focus:border-nova-400 bg-surface-secondary resize-none"
                />
              ) : (
                <input
                  name={name}
                  type={type || 'text'}
                  value={form[name]}
                  onChange={handleChange}
                  placeholder={placeholder}
                  required={required}
                  className="w-full px-4 py-2.5 text-sm border border-border rounded-xl focus:outline-none focus:ring-2 focus:ring-nova-500/20 focus:border-nova-400 bg-surface-secondary"
                />
              )}
            </div>
          ))}
        </div>

        <div className="flex items-center justify-end gap-3 mt-8 pt-6 border-t border-border">
          <button
            type="button"
            onClick={() => navigate(-1)}
            className="px-5 py-2.5 text-sm font-medium text-slate-600 hover:text-slate-800 rounded-xl hover:bg-slate-100 transition-colors"
          >
            Cancel
          </button>
          <button
            type="submit"
            disabled={saving}
            className="px-6 py-2.5 bg-gradient-to-r from-nova-500 to-nova-600 text-white text-sm font-semibold rounded-xl shadow-lg shadow-nova-500/25 hover:shadow-nova-500/40 hover:-translate-y-0.5 transition-all disabled:opacity-50 flex items-center gap-2"
          >
            {saving ? (
              <>
                <Loader2 className="w-4 h-4 animate-spin" />
                Saving...
              </>
            ) : (
              <>
                <UserPlus className="w-4 h-4" />
                Save Lead
              </>
            )}
          </button>
        </div>
      </form>
    </div>
  );
}
