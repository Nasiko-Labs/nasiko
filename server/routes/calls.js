import express from 'express';
import { getDB } from '../db/database.js';

const router = express.Router();

// GET /api/calls - Get all calls
router.get('/', (req, res) => {
  try {
    const db = getDB();
    const calls = db.prepare(`
      SELECT c.*, l.lead_name, l.company 
      FROM calls c 
      LEFT JOIN leads l ON c.lead_id = l.id 
      ORDER BY c.created_at DESC
    `).all();
    
    res.json(calls);
  } catch (error) {
    console.error('Error fetching calls:', error);
    res.status(500).json({ error: 'Failed to fetch calls' });
  }
});

// GET /api/calls/:id - Get single call
router.get('/:id', (req, res) => {
  try {
    const db = getDB();
    const call = db.prepare(`
      SELECT c.*, l.lead_name, l.company, l.email
      FROM calls c 
      LEFT JOIN leads l ON c.lead_id = l.id 
      WHERE c.id = ?
    `).get(req.params.id);
    
    if (!call) {
      return res.status(404).json({ error: 'Call not found' });
    }

    res.json(call);
  } catch (error) {
    console.error('Error fetching call:', error);
    res.status(500).json({ error: 'Failed to fetch call' });
  }
});

export default router;
