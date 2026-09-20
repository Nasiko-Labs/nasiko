import express from 'express';
import cors from 'cors';
import dotenv from 'dotenv';
import { initDB } from './db/database.js';
import leadsRouter from './routes/leads.js';
import callsRouter from './routes/calls.js';
import webhookRouter from './routes/webhook.js';
import dashboardRouter from './routes/dashboard.js';

dotenv.config({ path: '../.env' });

const app = express();
const PORT = process.env.PORT || 3001;

// Middleware
app.use(cors());
app.use(express.json());

// Initialize database
initDB();

// Routes
app.use('/api/leads', leadsRouter);
app.use('/api/calls', callsRouter);
app.use('/api/webhooks', webhookRouter);
app.use('/api/dashboard', dashboardRouter);

// Health check
app.get('/api/health', (req, res) => {
  res.json({ status: 'ok', timestamp: new Date().toISOString() });
});

app.listen(PORT, () => {
  console.log(`🚀 NOVA CRM Server running on http://localhost:${PORT}`);
});
