import axios from 'axios';

const api = axios.create({
  baseURL: '/api',
  timeout: 30000,
  headers: {
    'Content-Type': 'application/json'
  }
});

// Dashboard
export const getDashboard = () => api.get('/dashboard').then(r => r.data);

// Leads
export const getLeads = (params = {}) => api.get('/leads', { params }).then(r => r.data);
export const getLead = (id) => api.get(`/leads/${id}`).then(r => r.data);
export const createLead = (data) => api.post('/leads', data).then(r => r.data);
export const updateLead = (id, data) => api.put(`/leads/${id}`, data).then(r => r.data);
export const deleteLead = (id) => api.delete(`/leads/${id}`).then(r => r.data);

// Calls
export const initiateCall = (leadId) => api.post(`/leads/${leadId}/call`).then(r => r.data);
export const getCalls = () => api.get('/calls').then(r => r.data);
export const getCall = (id) => api.get(`/calls/${id}`).then(r => r.data);

export default api;
