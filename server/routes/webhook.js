import express from 'express';
import { getDB } from '../db/database.js';

const router = express.Router();

// POST /api/webhooks/nova - Receive NOVA post-call data
router.post('/nova', (req, res) => {
  try {
    const db = getDB();
    const payload = req.body;

    console.log('📞 Received NOVA webhook:', JSON.stringify(payload, null, 2));

    // Extract data from webhook payload
    // DronaHQ Voice Agent sends structured data through its Post-Webhook
    const {
      lead_id,
      call_id,
      phone,
      lead_name,
      customer_requirement,
      current_process,
      pain_point,
      business_impact,
      desired_outcome,
      budget,
      timeline,
      buying_criteria,
      decision_makers,
      interest_level,
      lead_score,
      call_status,
      call_summary,
      recommended_next_action,
      transcript,
      recording_url,
      duration
    } = payload;

    // Find the lead - try by lead_id first, then by phone number
    let lead = null;
    if (lead_id) {
      lead = db.prepare('SELECT * FROM leads WHERE id = ?').get(lead_id);
    }
    if (!lead && phone) {
      lead = db.prepare('SELECT * FROM leads WHERE phone = ?').get(phone);
    }
    if (!lead && lead_name) {
      lead = db.prepare('SELECT * FROM leads WHERE lead_name = ?').get(lead_name);
    }

    if (!lead) {
      console.warn('⚠️ No matching lead found for webhook data');
      return res.status(200).json({ 
        message: 'Webhook received but no matching lead found',
        received: true 
      });
    }

    // Update lead with call results
    const updateFields = {};
    if (customer_requirement) updateFields.customer_requirement = customer_requirement;
    if (current_process) updateFields.current_process = current_process;
    if (pain_point) updateFields.pain_point = pain_point;
    if (business_impact) updateFields.business_impact = business_impact;
    if (desired_outcome) updateFields.desired_outcome = desired_outcome;
    if (budget) updateFields.budget = budget;
    if (timeline) updateFields.timeline = timeline;
    if (buying_criteria) updateFields.buying_criteria = buying_criteria;
    if (decision_makers) updateFields.decision_makers = decision_makers;
    if (interest_level) updateFields.interest_level = interest_level;
    if (lead_score !== undefined) updateFields.lead_score = lead_score;
    if (call_summary) updateFields.call_summary = call_summary;
    if (recommended_next_action) updateFields.recommended_next_action = recommended_next_action;
    if (transcript) updateFields.transcript = transcript;
    if (recording_url) updateFields.recording_url = recording_url;
    
    updateFields.call_status = call_status || 'Completed';

    const setClauses = Object.keys(updateFields).map(key => `${key} = ?`).join(', ');
    const values = Object.values(updateFields);

    if (setClauses) {
      db.prepare(`UPDATE leads SET ${setClauses}, updated_at = CURRENT_TIMESTAMP WHERE id = ?`)
        .run(...values, lead.id);
    }

    // Create call history record
    db.prepare(`
      INSERT INTO calls (lead_id, call_id, phone, status, duration, transcript, recording_url, interest_level, lead_score, summary)
      VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
    `).run(
      lead.id,
      call_id || `nova-webhook-${Date.now()}`,
      phone || lead.phone,
      call_status || 'completed',
      duration || 0,
      transcript || null,
      recording_url || null,
      interest_level || null,
      lead_score || null,
      call_summary || null
    );

    console.log(`✅ Lead ${lead.id} updated with NOVA call results`);

    res.status(200).json({ 
      message: 'Webhook processed successfully',
      lead_id: lead.id,
      received: true 
    });
  } catch (error) {
    console.error('❌ Webhook processing error:', error);
    // Still return 200 to prevent DronaHQ from retrying
    res.status(200).json({ 
      message: 'Webhook received with processing error',
      error: error.message,
      received: true 
    });
  }
});

export default router;
