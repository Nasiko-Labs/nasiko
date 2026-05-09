import React, { useState } from 'react';
import { Play, Loader2 } from 'lucide-react';
import { Card, CardContent, CardHeader, CardTitle, CardDescription } from '../ui/card';
import axios from 'axios';

const API_URL = 'http://localhost:3000/api';

export default function TestPanel({ agents = ['mock-translator', 'mock-analyzer', 'mock-summarizer'] }) {
  const [selectedAgent, setSelectedAgent] = useState(agents[0] || 'mock-translator');
  const [query, setQuery] = useState('Hello world');
  const [burstCount, setBurstCount] = useState(1);
  const [loading, setLoading] = useState(false);
  const [lastResult, setLastResult] = useState(null);

  const handleFireRequest = async () => {
    setLoading(true);
    setLastResult(null);
    try {
      const requests = Array.from({ length: burstCount }).map(() =>
        axios.post(`${API_URL}/process`, {
          agent: selectedAgent,
          query: query + (burstCount > 1 ? ` ${Math.random().toString(36).substring(7)}` : '')
        })
      );
      
      const start = Date.now();
      const results = await Promise.allSettled(requests);
      const end = Date.now();
      
      const success = results.filter(r => r.status === 'fulfilled').length;
      const failed = results.filter(r => r.status === 'rejected').length;
      
      setLastResult({
        success,
        failed,
        time: end - start,
        isBurst: burstCount > 1
      });
    } catch (err) {
      console.error(err);
    } finally {
      setLoading(false);
    }
  };

  return (
    <Card className="glass-card mt-6 border-blue-500/20">
      <CardHeader className="pb-3">
        <CardTitle className="text-lg flex items-center gap-2">
          <Play className="w-5 h-5 text-blue-400" />
          Test Traffic Generator
        </CardTitle>
        <CardDescription>Send test requests to agents to see rate limiting and caching in action.</CardDescription>
      </CardHeader>
      <CardContent>
        <div className="flex flex-col md:flex-row gap-4 items-end">
          <div className="space-y-1.5 flex-1">
            <label className="text-xs font-medium text-muted-foreground">Target Agent</label>
            <select 
              className="flex h-10 w-full items-center justify-between rounded-md border border-white/10 bg-background/50 px-3 py-2 text-sm ring-offset-background placeholder:text-muted-foreground focus:outline-none focus:ring-2 focus:ring-ring focus:ring-offset-2 disabled:cursor-not-allowed disabled:opacity-50"
              value={selectedAgent}
              onChange={(e) => setSelectedAgent(e.target.value)}
            >
              {agents.map(a => <option key={a} value={a}>{a}</option>)}
            </select>
          </div>
          
          <div className="space-y-1.5 flex-[2]">
            <label className="text-xs font-medium text-muted-foreground">Query Payload</label>
            <input 
              type="text" 
              className="flex h-10 w-full rounded-md border border-white/10 bg-background/50 px-3 py-2 text-sm ring-offset-background file:border-0 file:bg-transparent file:text-sm file:font-medium placeholder:text-muted-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 disabled:cursor-not-allowed disabled:opacity-50"
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              placeholder="Enter text to process..."
            />
          </div>
          
          <div className="space-y-1.5 w-24">
            <label className="text-xs font-medium text-muted-foreground">Burst Count</label>
            <input 
              type="number" 
              min="1" max="50"
              className="flex h-10 w-full rounded-md border border-white/10 bg-background/50 px-3 py-2 text-sm ring-offset-background file:border-0 file:bg-transparent file:text-sm file:font-medium placeholder:text-muted-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 disabled:cursor-not-allowed disabled:opacity-50"
              value={burstCount}
              onChange={(e) => setBurstCount(parseInt(e.target.value) || 1)}
            />
          </div>
          
          <button 
            onClick={handleFireRequest}
            disabled={loading}
            className="h-10 px-6 py-2 bg-blue-600 hover:bg-blue-700 text-white rounded-md text-sm font-medium transition-colors flex items-center justify-center min-w-[120px]"
          >
            {loading ? <Loader2 className="w-4 h-4 animate-spin" /> : "Send Request"}
          </button>
        </div>
        
        {lastResult && (
          <div className="mt-4 p-3 rounded-md bg-white/5 text-sm flex gap-6">
            <span className="text-muted-foreground">Last Run ({lastResult.time}ms):</span>
            {lastResult.success > 0 && <span className="text-emerald-400 font-medium">{lastResult.success} Successful</span>}
            {lastResult.failed > 0 && <span className="text-red-400 font-medium">{lastResult.failed} Failed</span>}
            {lastResult.isBurst && <span className="text-xs text-muted-foreground ml-auto self-center">(Burst added random entropy to bypass cache)</span>}
          </div>
        )}
      </CardContent>
    </Card>
  );
}
