import React, { useState } from 'react';
import { Activity, Clock, Database, Layers, ShieldAlert, Zap, RefreshCw, Settings2 } from 'lucide-react';
import { LineChart, Line, XAxis, YAxis, CartesianGrid, Tooltip, ResponsiveContainer, AreaChart, Area } from 'recharts';
import { Card, CardContent, CardHeader, CardTitle, CardDescription } from './ui/card';
import { Badge } from './ui/badge';
import { useDashboardData } from '../hooks/useDashboardData';
import { cn } from '../lib/utils';
import TestPanel from './controls/TestPanel';

export default function Dashboard() {
  const { globalStats, queueStatus, recentRequests, agentStats, timeSeries, limits, flushCache, updateLimit } = useDashboardData();
  const [selectedAgent, setSelectedAgent] = useState(null);
  const [editingLimit, setEditingLimit] = useState(null); // { agent: string, tokens: number, rate: number }

  if (!globalStats) {
    return (
      <div className="flex items-center justify-center h-screen bg-background text-foreground">
        <div className="flex flex-col items-center gap-4">
          <RefreshCw className="w-8 h-8 animate-spin text-primary" />
          <p className="text-muted-foreground animate-pulse">Connecting to Request Layer...</p>
        </div>
      </div>
    );
  }

  return (
    <div className="min-h-screen p-6 md:p-8 space-y-8 max-w-7xl mx-auto">
      {/* Header */}
      <header className="flex flex-col md:flex-row md:items-center justify-between gap-4">
        <div>
          <h1 className="text-3xl font-bold tracking-tight bg-clip-text text-transparent bg-gradient-to-r from-primary to-blue-400">
            Nasiko Request Layer
          </h1>
          <p className="text-muted-foreground mt-1">Real-time traffic control and caching</p>
        </div>
        <div className="flex items-center gap-3">
          <Badge variant="outline" className="px-3 py-1 bg-card/50 backdrop-blur-sm">
            <span className="w-2 h-2 rounded-full bg-emerald-500 mr-2 animate-pulse"></span>
            System Healthy
          </Badge>
          <button 
            onClick={flushCache}
            className="flex items-center gap-2 px-4 py-2 bg-secondary text-secondary-foreground hover:bg-secondary/80 rounded-md text-sm font-medium transition-colors"
          >
            <Database className="w-4 h-4" />
            Flush Cache
          </button>
        </div>
      </header>

      {/* Hero Stats */}
      <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-4 gap-4">
        <StatCard 
          title="Total Requests" 
          value={globalStats.totalRequests.toLocaleString()} 
          icon={<Activity className="w-5 h-5 text-blue-500" />}
          trend="+12% from last hour"
        />
        <StatCard 
          title="Cache Hit Rate" 
          value={`${globalStats.cacheHitRate}%`} 
          icon={<Zap className="w-5 h-5 text-amber-500" />}
          description={`${globalStats.cacheHits} total hits`}
        />
        <StatCard 
          title="Queued Traffic" 
          value={globalStats.queuedRequests.toLocaleString()} 
          icon={<Layers className="w-5 h-5 text-indigo-500" />}
          description="Excess traffic handled"
        />
        <StatCard 
          title="Success Rate" 
          value={`${globalStats.successRate}%`} 
          icon={<ShieldAlert className="w-5 h-5 text-emerald-500" />}
          description={`${globalStats.failedRequests} failed`}
        />
      </div>

      <div className="grid grid-cols-1 lg:grid-cols-3 gap-6">
        {/* Main Chart */}
        <Card className="lg:col-span-2 glass-card">
          <CardHeader>
            <CardTitle>Throughput & Cache Performance</CardTitle>
            <CardDescription>Live traffic over the last 30 minutes</CardDescription>
          </CardHeader>
          <CardContent>
            <div className="h-[300px] w-full">
              <ResponsiveContainer width="100%" height="100%">
                <AreaChart data={timeSeries} margin={{ top: 10, right: 10, left: -20, bottom: 0 }}>
                  <defs>
                    <linearGradient id="colorReq" x1="0" y1="0" x2="0" y2="1">
                      <stop offset="5%" stopColor="#3b82f6" stopOpacity={0.3}/>
                      <stop offset="95%" stopColor="#3b82f6" stopOpacity={0}/>
                    </linearGradient>
                    <linearGradient id="colorCache" x1="0" y1="0" x2="0" y2="1">
                      <stop offset="5%" stopColor="#f59e0b" stopOpacity={0.3}/>
                      <stop offset="95%" stopColor="#f59e0b" stopOpacity={0}/>
                    </linearGradient>
                  </defs>
                  <CartesianGrid strokeDasharray="3 3" stroke="rgba(255,255,255,0.1)" vertical={false} />
                  <XAxis 
                    dataKey="timestamp" 
                    tickFormatter={(tick) => new Date(tick).toLocaleTimeString([], {hour: '2-digit', minute:'2-digit'})} 
                    stroke="#888888" 
                    fontSize={12} 
                    tickLine={false} 
                    axisLine={false}
                  />
                  <YAxis stroke="#888888" fontSize={12} tickLine={false} axisLine={false} />
                  <Tooltip 
                    contentStyle={{ backgroundColor: 'rgba(15, 15, 20, 0.9)', borderColor: 'rgba(255,255,255,0.1)', borderRadius: '8px' }}
                    labelFormatter={(label) => new Date(label).toLocaleTimeString()}
                  />
                  <Area type="monotone" dataKey="requests" name="Total Requests" stroke="#3b82f6" strokeWidth={2} fillOpacity={1} fill="url(#colorReq)" />
                  <Area type="monotone" dataKey="cacheHits" name="Cache Hits" stroke="#f59e0b" strokeWidth={2} fillOpacity={1} fill="url(#colorCache)" />
                </AreaChart>
              </ResponsiveContainer>
            </div>
          </CardContent>
        </Card>

        {/* Live Request Feed */}
        <Card className="glass-card flex flex-col">
          <CardHeader className="pb-3">
            <CardTitle>Live Traffic Feed</CardTitle>
            <CardDescription>Recent incoming requests</CardDescription>
          </CardHeader>
          <CardContent className="flex-1 overflow-hidden">
            <div className="space-y-3 h-[300px] overflow-y-auto pr-2 custom-scrollbar">
              {recentRequests.map((req, i) => (
                <div key={i} className="flex items-center justify-between p-3 rounded-lg bg-card/50 border border-white/5 text-sm animate-slide-up">
                  <div className="flex items-center gap-3 truncate">
                    {req.cached ? (
                      <div className="w-8 h-8 rounded bg-amber-500/20 flex items-center justify-center text-amber-500 shrink-0">
                        <Zap className="w-4 h-4" />
                      </div>
                    ) : req.queued ? (
                      <div className="w-8 h-8 rounded bg-indigo-500/20 flex items-center justify-center text-indigo-500 shrink-0">
                        <Clock className="w-4 h-4" />
                      </div>
                    ) : (
                      <div className="w-8 h-8 rounded bg-blue-500/20 flex items-center justify-center text-blue-500 shrink-0">
                        <Activity className="w-4 h-4" />
                      </div>
                    )}
                    <div className="truncate">
                      <p className="font-medium text-foreground truncate">{req.agent}</p>
                      <p className="text-xs text-muted-foreground truncate">{new Date(req.timestamp).toLocaleTimeString()}</p>
                    </div>
                  </div>
                  <div className="text-right shrink-0 ml-2">
                    <p className="font-medium">{req.latency}ms</p>
                    {req.cached && <Badge variant="warning" className="mt-1 text-[10px] px-1 py-0 h-4 leading-4">HIT</Badge>}
                  </div>
                </div>
              ))}
              {recentRequests.length === 0 && (
                <div className="text-center text-muted-foreground text-sm py-10">No recent requests</div>
              )}
            </div>
          </CardContent>
        </Card>
      </div>

      {/* Agents Status */}
      <div>
        <h2 className="text-xl font-semibold mb-4">Agent Fleet Status</h2>
        <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-4">
          {Object.keys(agentStats).length > 0 ? (
            Object.entries(agentStats).map(([name, stats]) => {
              const queue = queueStatus[name] || { waiting: 0, avgWaitTime: 0 };
              const limit = limits[name] || { maxTokens: 10, refillRate: 2 };
              
              return (
                <Card key={name} className="glass-card hover:border-primary/50 transition-colors">
                  <CardHeader className="pb-2">
                    <div className="flex justify-between items-start">
                      <CardTitle className="text-lg">{name}</CardTitle>
                      {queue.waiting > 0 ? (
                        <Badge variant="warning" className="animate-pulse">{queue.waiting} queued</Badge>
                      ) : (
                        <Badge variant="success">Available</Badge>
                      )}
                    </div>
                    <CardDescription>
                      {stats.totalRequests} reqs • {stats.cacheHitRate}% cache hit
                    </CardDescription>
                  </CardHeader>
                  <CardContent>
                    <div className="space-y-4 pt-2">
                      <div className="grid grid-cols-2 gap-2 text-sm">
                        <div className="p-2 rounded bg-secondary/50">
                          <p className="text-muted-foreground text-xs">Avg Latency</p>
                          <p className="font-medium">{stats.avgLatency}ms</p>
                        </div>
                        <div className="p-2 rounded bg-secondary/50">
                          <p className="text-muted-foreground text-xs">P95 Latency</p>
                          <p className="font-medium">{stats.p95Latency}ms</p>
                        </div>
                      </div>
                      
                      <div className="pt-2 border-t border-white/10">
                        <div className="flex flex-col text-sm mb-2">
                          <div className="flex justify-between items-center mb-1">
                            <span className="text-muted-foreground flex items-center gap-1"><Settings2 className="w-3 h-3"/> Rate Limit</span>
                            {editingLimit?.agent === name ? (
                              <div className="flex items-center gap-1">
                                <input 
                                  type="number" 
                                  className="w-12 h-6 px-1 bg-background border border-white/10 rounded text-xs text-center" 
                                  value={editingLimit.tokens} 
                                  onChange={e => setEditingLimit({...editingLimit, tokens: parseInt(e.target.value) || 1})}
                                />
                                <span className="text-xs">/</span>
                                <input 
                                  type="number" 
                                  className="w-12 h-6 px-1 bg-background border border-white/10 rounded text-xs text-center" 
                                  value={editingLimit.rate} 
                                  onChange={e => setEditingLimit({...editingLimit, rate: parseInt(e.target.value) || 1})}
                                />
                                <span className="text-xs">/s</span>
                              </div>
                            ) : (
                              <span>{limit.maxTokens} burst / {limit.refillRate}/s</span>
                            )}
                          </div>
                          <div className="flex justify-end">
                            {editingLimit?.agent === name ? (
                              <div className="flex gap-2">
                                <button onClick={() => setEditingLimit(null)} className="text-[10px] text-muted-foreground hover:text-white px-2 py-1 rounded bg-secondary/50">Cancel</button>
                                <button 
                                  onClick={() => {
                                    updateLimit(name, { maxTokens: editingLimit.tokens, refillRate: editingLimit.rate });
                                    setEditingLimit(null);
                                  }} 
                                  className="text-[10px] text-primary hover:text-primary/80 px-2 py-1 rounded bg-primary/20"
                                >
                                  Save
                                </button>
                              </div>
                            ) : (
                              <button onClick={() => setEditingLimit({ agent: name, tokens: limit.maxTokens, rate: limit.refillRate })} className="text-[10px] text-blue-400 hover:text-blue-300">Edit Limits</button>
                            )}
                          </div>
                        </div>
                        <div className="flex justify-between items-center text-sm">
                          <span className="text-muted-foreground flex items-center gap-1"><Clock className="w-3 h-3"/> Queue Wait</span>
                          <span>{queue.avgWaitTime}ms avg</span>
                        </div>
                      </div>
                    </div>
                  </CardContent>
                </Card>
              );
            })
          ) : (
            <div className="col-span-full py-10 text-center border rounded-xl bg-card border-dashed">
              <p className="text-muted-foreground">No agents have processed requests yet. Use the Test Panel below to send some traffic!</p>
            </div>
          )}
        </div>
      </div>

      {/* Test Panel */}
      <TestPanel agents={Object.keys(agentStats).length > 0 ? Object.keys(agentStats) : ['mock-translator', 'mock-analyzer', 'mock-summarizer']} />
    </div>
  );
}

function StatCard({ title, value, icon, description, trend }) {
  return (
    <Card className="glass-panel">
      <CardHeader className="flex flex-row items-center justify-between space-y-0 pb-2">
        <CardTitle className="text-sm font-medium text-muted-foreground">{title}</CardTitle>
        {icon}
      </CardHeader>
      <CardContent>
        <div className="text-2xl font-bold">{value}</div>
        {(description || trend) && (
          <p className={cn("text-xs mt-1", trend ? "text-emerald-500" : "text-muted-foreground")}>
            {trend || description}
          </p>
        )}
      </CardContent>
    </Card>
  );
}
