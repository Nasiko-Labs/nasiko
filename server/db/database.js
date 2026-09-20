import Database from 'better-sqlite3';
import path from 'path';
import { fileURLToPath } from 'url';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const DB_PATH = path.join(__dirname, 'nova.db');

let db;

export function getDB() {
  if (!db) {
    db = new Database(DB_PATH);
    db.pragma('journal_mode = WAL');
    db.pragma('foreign_keys = ON');
  }
  return db;
}

export function initDB() {
  const database = getDB();

  database.exec(`
    CREATE TABLE IF NOT EXISTS leads (
      id INTEGER PRIMARY KEY AUTOINCREMENT,
      lead_name TEXT NOT NULL,
      company TEXT,
      phone TEXT,
      email TEXT,
      job_title TEXT,
      industry TEXT,
      customer_requirement TEXT,
      current_process TEXT,
      pain_point TEXT,
      business_impact TEXT,
      desired_outcome TEXT,
      existing_solution TEXT,
      budget TEXT,
      timeline TEXT,
      buying_criteria TEXT,
      decision_makers TEXT,
      interest_level TEXT DEFAULT 'Unknown',
      lead_score INTEGER DEFAULT 0,
      call_status TEXT DEFAULT 'Not Called',
      call_summary TEXT,
      recommended_next_action TEXT,
      transcript TEXT,
      recording_url TEXT,
      notes TEXT,
      created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
      updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
    );

    CREATE TABLE IF NOT EXISTS calls (
      id INTEGER PRIMARY KEY AUTOINCREMENT,
      lead_id INTEGER NOT NULL,
      call_id TEXT,
      phone TEXT,
      status TEXT DEFAULT 'initiated',
      duration INTEGER DEFAULT 0,
      transcript TEXT,
      recording_url TEXT,
      interest_level TEXT,
      lead_score INTEGER,
      summary TEXT,
      created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
      FOREIGN KEY (lead_id) REFERENCES leads(id) ON DELETE CASCADE
    );
  `);

  console.log('✅ Database initialized');
}
