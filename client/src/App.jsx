import { BrowserRouter, Routes, Route } from 'react-router-dom';
import Layout from './components/Layout';
import Dashboard from './pages/Dashboard';
import Leads from './pages/Leads';
import AddLead from './pages/AddLead';
import LeadDetails from './pages/LeadDetails';
import CallHistory from './pages/CallHistory';

export default function App() {
  return (
    <BrowserRouter>
      <Routes>
        <Route path="/" element={<Layout />}>
          <Route index element={<Dashboard />} />
          <Route path="leads" element={<Leads />} />
          <Route path="leads/new" element={<AddLead />} />
          <Route path="leads/:id" element={<LeadDetails />} />
          <Route path="calls" element={<CallHistory />} />
        </Route>
      </Routes>
    </BrowserRouter>
  );
}
