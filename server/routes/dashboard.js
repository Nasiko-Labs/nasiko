import express from 'express';
import { getDB } from '../db/database.js';

const router = express.Router();

// GET /api/dashboard - Get dashboard stats
router.get('/', (req, res) => {
  try {
    const db = getDB();

    const totalLeads = db.prepare('SELECT COUNT(*) as count FROM leads').get().count;
    const callsCompleted = db.prepare("SELECT COUNT(*) as count FROM leads WHERE call_status = 'Completed'").get().count;
    const highInterest = db.prepare("SELECT COUNT(*) as count FROM leads WHERE interest_level = 'High'").get().count;
    
    const avgScoreResult = db.prepare('SELECT AVG(lead_score) as avg FROM leads WHERE lead_score > 0').get();
    const avgScore = Math.round(avgScoreResult.avg || 0);

    const recentLeads = db.prepare('SELECT id, lead_name, company, interest_level, lead_score, call_status, created_at FROM leads ORDER BY created_at DESC LIMIT 5').all();

    const recentCalls = db.prepare(`
      SELECT c.id, c.phone, c.status, c.duration, c.created_at, c.interest_level, c.lead_score,
             l.lead_name, l.company
      FROM calls c
      LEFT JOIN leads l ON c.lead_id = l.id
      ORDER BY c.created_at DESC LIMIT 5
    `).all();

    const interestDistribution = db.prepare(`
      SELECT interest_level, COUNT(*) as count 
      FROM leads 
      GROUP BY interest_level
    `).all();

    const scoreDistribution = db.prepare(`
      SELECT 
        CASE 
          WHEN lead_score >= 80 THEN 'Hot (80-100)'
          WHEN lead_score >= 60 THEN 'Warm (60-79)'
          WHEN lead_score >= 40 THEN 'Cool (40-59)'
          WHEN lead_score > 0 THEN 'Cold (1-39)'
          ELSE 'Unscored'
        END as range,
        COUNT(*) as count
      FROM leads
      GROUP BY range
    `).all();

    const callStatusDistribution = db.prepare(`
      SELECT call_status, COUNT(*) as count 
      FROM leads 
      GROUP BY call_status
    `).all();

    res.json({
      stats: {
        totalLeads,
        callsCompleted,
        highInterest,
        avgScore
      },
      recentLeads,
      recentCalls,
      interestDistribution,
      scoreDistribution,
      callStatusDistribution
    });
  } catch (error) {
    console.error('Error fetching dashboard:', error);
    res.status(500).json({ error: 'Failed to fetch dashboard data' });
  }
});

export default router;
