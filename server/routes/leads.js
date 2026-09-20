import express from 'express';
import { getDB } from '../db/database.js';
import { initiateCall } from '../services/dronahq.js';

const router = express.Router();

// GET /api/leads - Get all leads
router.get('/', (req, res) => {
  try {
    const db = getDB();
    const { search, interest_level, call_status, sort_by, order } = req.query;
    
    let query = 'SELECT * FROM leads WHERE 1=1';
    const params = [];

    if (search) {
      query += ' AND (lead_name LIKE ? OR company LIKE ? OR email LIKE ? OR phone LIKE ?)';
      const searchTerm = `%${search}%`;
      params.push(searchTerm, searchTerm, searchTerm, searchTerm);
    }

    if (interest_level && interest_level !== 'All') {
      query += ' AND interest_level = ?';
      params.push(interest_level);
    }

    if (call_status && call_status !== 'All') {
      query += ' AND call_status = ?';
      params.push(call_status);
    }

    const validSortColumns = ['lead_score', 'created_at', 'lead_name', 'company', 'interest_level'];
    const sortColumn = validSortColumns.includes(sort_by) ? sort_by : 'created_at';
    const sortOrder = order === 'ASC' ? 'ASC' : 'DESC';
    query += ` ORDER BY ${sortColumn} ${sortOrder}`;

    const leads = db.prepare(query).all(...params);
    res.json(leads);
  } catch (error) {
    console.error('Error fetching leads:', error);
    res.status(500).json({ error: 'Failed to fetch leads' });
  }
});

// GET /api/leads/:id - Get single lead
router.get('/:id', (req, res) => {
  try {
    const db = getDB();
    const lead = db.prepare('SELECT * FROM leads WHERE id = ?').get(req.params.id);
    
    if (!lead) {
      return res.status(404).json({ error: 'Lead not found' });
    }

    // Get associated calls
    const calls = db.prepare('SELECT * FROM calls WHERE lead_id = ? ORDER BY created_at DESC').all(req.params.id);
    
    res.json({ ...lead, calls });
  } catch (error) {
    console.error('Error fetching lead:', error);
    res.status(500).json({ error: 'Failed to fetch lead' });
  }
});

// POST /api/leads - Create new lead
router.post('/', (req, res) => {
  try {
    const db = getDB();
    const {
      lead_name, company, phone, email, job_title, industry,
      customer_requirement, pain_point, budget, timeline, notes
    } = req.body;

    if (!lead_name || !lead_name.trim()) {
      return res.status(400).json({ error: 'Lead name is required' });
    }

    const result = db.prepare(`
      INSERT INTO leads (lead_name, company, phone, email, job_title, industry,
        customer_requirement, pain_point, budget, timeline, notes)
      VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
    `).run(
      lead_name.trim(), company?.trim() || null, phone?.trim() || null,
      email?.trim() || null, job_title?.trim() || null, industry?.trim() || null,
      customer_requirement?.trim() || null, pain_point?.trim() || null,
      budget?.trim() || null, timeline?.trim() || null, notes?.trim() || null
    );

    const newLead = db.prepare('SELECT * FROM leads WHERE id = ?').get(result.lastInsertRowid);
    res.status(201).json(newLead);
  } catch (error) {
    console.error('Error creating lead:', error);
    res.status(500).json({ error: 'Failed to create lead' });
  }
});

// PUT /api/leads/:id - Update lead
router.put('/:id', (req, res) => {
  try {
    const db = getDB();
    const lead = db.prepare('SELECT * FROM leads WHERE id = ?').get(req.params.id);
    
    if (!lead) {
      return res.status(404).json({ error: 'Lead not found' });
    }

    const fields = [
      'lead_name', 'company', 'phone', 'email', 'job_title', 'industry',
      'customer_requirement', 'current_process', 'pain_point', 'business_impact',
      'desired_outcome', 'existing_solution', 'budget', 'timeline',
      'buying_criteria', 'decision_makers', 'interest_level', 'lead_score',
      'call_status', 'call_summary', 'recommended_next_action', 'transcript',
      'recording_url', 'notes'
    ];

    const updates = [];
    const values = [];

    for (const field of fields) {
      if (req.body[field] !== undefined) {
        updates.push(`${field} = ?`);
        values.push(req.body[field]);
      }
    }

    if (updates.length === 0) {
      return res.status(400).json({ error: 'No fields to update' });
    }

    updates.push('updated_at = CURRENT_TIMESTAMP');
    values.push(req.params.id);

    db.prepare(`UPDATE leads SET ${updates.join(', ')} WHERE id = ?`).run(...values);
    
    const updatedLead = db.prepare('SELECT * FROM leads WHERE id = ?').get(req.params.id);
    res.json(updatedLead);
  } catch (error) {
    console.error('Error updating lead:', error);
    res.status(500).json({ error: 'Failed to update lead' });
  }
});

// DELETE /api/leads/:id - Delete lead
router.delete('/:id', (req, res) => {
  try {
    const db = getDB();
    const lead = db.prepare('SELECT * FROM leads WHERE id = ?').get(req.params.id);
    
    if (!lead) {
      return res.status(404).json({ error: 'Lead not found' });
    }

    db.prepare('DELETE FROM leads WHERE id = ?').run(req.params.id);
    res.json({ message: 'Lead deleted successfully' });
  } catch (error) {
    console.error('Error deleting lead:', error);
    res.status(500).json({ error: 'Failed to delete lead' });
  }
});

// POST /api/leads/:id/call - Initiate NOVA call
router.post('/:id/call', async (req, res) => {
  try {
    const db = getDB();
    const lead = db.prepare('SELECT * FROM leads WHERE id = ?').get(req.params.id);
    
    if (!lead) {
      return res.status(404).json({ error: 'Lead not found' });
    }

    if (!lead.phone || !lead.phone.trim()) {
      return res.status(400).json({ error: 'Lead does not have a phone number' });
    }

    // Update call status
    db.prepare('UPDATE leads SET call_status = ?, updated_at = CURRENT_TIMESTAMP WHERE id = ?')
      .run('Calling', req.params.id);

    // Prepare context for NOVA
    const context = {
      lead_name: lead.lead_name,
      company: lead.company,
      industry: lead.industry,
      job_title: lead.job_title,
      customer_requirement: lead.customer_requirement,
      pain_point: lead.pain_point,
      notes: lead.notes
    };

    // Initiate call via DronaHQ
    const callResult = await initiateCall(lead.phone, context, lead.id);

    // Create call record
    const callRecord = db.prepare(`
      INSERT INTO calls (lead_id, call_id, phone, status)
      VALUES (?, ?, ?, ?)
    `).run(lead.id, callResult.call_id || `nova-${Date.now()}`, lead.phone, 'initiated');

    res.json({
      message: 'Call initiated successfully',
      call_id: callResult.call_id,
      lead_id: lead.id
    });
  } catch (error) {
    console.error('Error initiating call:', error);
    
    // Reset call status on failure
    const db = getDB();
    db.prepare('UPDATE leads SET call_status = ?, updated_at = CURRENT_TIMESTAMP WHERE id = ?')
      .run('Failed', req.params.id);

    res.status(500).json({ 
      error: 'Failed to initiate call',
      details: error.message 
    });
  }
});

export default router;
