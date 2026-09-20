import { getDB } from './database.js';
import { initDB } from './database.js';

// Initialize the database first
initDB();

const db = getDB();

const sampleLeads = [
  {
    lead_name: 'Rahul Sharma',
    company: 'ABC Technologies',
    phone: '+919876543210',
    email: 'rahul.sharma@abctech.com',
    job_title: 'Operations Manager',
    industry: 'Information Technology',
    customer_requirement: 'AI-powered sales automation platform',
    current_process: 'Manual cold calling with spreadsheet tracking',
    pain_point: 'Low conversion rate and high sales team burnout',
    business_impact: 'Losing 40% potential revenue due to unqualified leads',
    desired_outcome: 'Automated lead qualification with 2x conversion',
    budget: '₹5,00,000',
    timeline: '1 month',
    notes: 'Met at TechSummit 2026. Very interested in AI automation. Has budget approval.',
    interest_level: 'High',
    lead_score: 85,
    call_status: 'Completed',
    call_summary: 'Rahul showed strong interest in NOVA for automating their outbound sales process. Currently managing 15 sales reps doing manual calls. Wants to pilot with 5 reps first.',
    recommended_next_action: 'Schedule product demo with sales team lead'
  },
  {
    lead_name: 'Priya Patel',
    company: 'GrowthBox Solutions',
    phone: '+919812345678',
    email: 'priya@growthbox.in',
    job_title: 'VP of Sales',
    industry: 'SaaS',
    customer_requirement: 'Intelligent lead scoring and qualification system',
    current_process: 'Using basic CRM with manual lead scoring',
    pain_point: 'Sales team spends 60% time on unqualified leads',
    business_impact: 'Missing quarterly targets by 25%',
    desired_outcome: 'AI-driven lead prioritization and automated first-touch',
    budget: '₹8,00,000',
    timeline: '2 weeks',
    notes: 'Referred by Rahul Sharma. Urgently needs solution before Q4.',
    interest_level: 'High',
    lead_score: 92,
    call_status: 'Completed',
    call_summary: 'Priya is very motivated to implement NOVA. Their current process is costing them significant revenue. Ready to move fast with implementation.',
    recommended_next_action: 'Send proposal and schedule onboarding call'
  },
  {
    lead_name: 'Vikram Desai',
    company: 'Pinnacle Enterprises',
    phone: '+919723456789',
    email: 'vikram.desai@pinnacle.co.in',
    job_title: 'Business Development Head',
    industry: 'Real Estate',
    customer_requirement: 'Automated follow-up system for property inquiries',
    current_process: 'Manual follow-ups through WhatsApp and phone calls',
    pain_point: 'Missing 50% of follow-ups due to high volume',
    business_impact: 'Estimated ₹2Cr lost annually in missed deals',
    desired_outcome: 'Automated voice follow-ups within 5 minutes of inquiry',
    budget: '₹3,00,000',
    timeline: '3 months',
    notes: 'Large real estate firm with 200+ agents.',
    interest_level: 'Medium',
    lead_score: 65,
    call_status: 'Not Called',
    call_summary: null,
    recommended_next_action: 'Initial qualification call needed'
  },
  {
    lead_name: 'Ananya Krishnan',
    company: 'FinEdge Capital',
    phone: '+919634567890',
    email: 'ananya.k@finedge.com',
    job_title: 'Chief Revenue Officer',
    industry: 'Financial Services',
    customer_requirement: 'Voice AI for loan pre-qualification calls',
    current_process: 'Call center with 50 agents doing pre-qualification',
    pain_point: 'High agent attrition and inconsistent qualifying criteria',
    business_impact: 'Agent training costs ₹50L yearly, 30% attrition rate',
    desired_outcome: 'Standardized AI qualification reducing agent dependency',
    budget: '₹12,00,000',
    timeline: '1 month',
    notes: 'Enterprise prospect. Needs compliance documentation.',
    interest_level: 'High',
    lead_score: 78,
    call_status: 'Not Called',
    call_summary: null,
    recommended_next_action: 'Schedule discovery call with compliance team'
  },
  {
    lead_name: 'Arjun Mehta',
    company: 'CloudNine Hosting',
    phone: '+919545678901',
    email: 'arjun@cloudnine.io',
    job_title: 'Sales Manager',
    industry: 'Cloud Services',
    customer_requirement: 'AI assistant for upselling existing customers',
    current_process: 'Monthly manual review of customer accounts',
    pain_point: 'Missing upsell opportunities, low customer engagement',
    business_impact: 'Only capturing 10% of potential upsell revenue',
    desired_outcome: 'Proactive AI outreach for upgrade opportunities',
    budget: '₹2,00,000',
    timeline: '2 months',
    notes: 'Small team but high growth potential. Wants to start with pilot.',
    interest_level: 'Low',
    lead_score: 42,
    call_status: 'Failed',
    call_summary: 'Call could not be completed. Number was busy.',
    recommended_next_action: 'Retry call during business hours'
  }
];

// Clear existing data
db.exec('DELETE FROM calls');
db.exec('DELETE FROM leads');
db.exec("DELETE FROM sqlite_sequence WHERE name='leads'");
db.exec("DELETE FROM sqlite_sequence WHERE name='calls'");

const insertLead = db.prepare(`
  INSERT INTO leads (
    lead_name, company, phone, email, job_title, industry,
    customer_requirement, current_process, pain_point, business_impact,
    desired_outcome, budget, timeline, notes, interest_level,
    lead_score, call_status, call_summary, recommended_next_action
  ) VALUES (
    @lead_name, @company, @phone, @email, @job_title, @industry,
    @customer_requirement, @current_process, @pain_point, @business_impact,
    @desired_outcome, @budget, @timeline, @notes, @interest_level,
    @lead_score, @call_status, @call_summary, @recommended_next_action
  )
`);

const insertCall = db.prepare(`
  INSERT INTO calls (lead_id, call_id, phone, status, duration, summary, interest_level, lead_score)
  VALUES (@lead_id, @call_id, @phone, @status, @duration, @summary, @interest_level, @lead_score)
`);

const insertMany = db.transaction(() => {
  for (const lead of sampleLeads) {
    const result = insertLead.run(lead);
    
    // Add call records for leads that have been called
    if (lead.call_status === 'Completed') {
      insertCall.run({
        lead_id: result.lastInsertRowid,
        call_id: `nova-${Date.now()}-${Math.random().toString(36).substr(2, 6)}`,
        phone: lead.phone,
        status: 'completed',
        duration: Math.floor(Math.random() * 300) + 120,
        summary: lead.call_summary,
        interest_level: lead.interest_level,
        lead_score: lead.lead_score
      });
    } else if (lead.call_status === 'Failed') {
      insertCall.run({
        lead_id: result.lastInsertRowid,
        call_id: `nova-${Date.now()}-${Math.random().toString(36).substr(2, 6)}`,
        phone: lead.phone,
        status: 'failed',
        duration: 0,
        summary: lead.call_summary,
        interest_level: lead.interest_level,
        lead_score: lead.lead_score
      });
    }
  }
});

insertMany();

console.log('✅ Database seeded with 5 sample leads');
console.log('📊 Leads inserted:', db.prepare('SELECT COUNT(*) as count FROM leads').get());
console.log('📞 Calls inserted:', db.prepare('SELECT COUNT(*) as count FROM calls').get());
