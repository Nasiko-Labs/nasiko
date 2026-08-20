import React, { useState, useEffect } from 'react';
import { 
  Activity, 
  Clock, 
  CheckCircle2, 
  XCircle, 
  Zap, 
  Server,
  TrendingUp,
  TrendingDown,
  LayoutDashboard,
  Settings
} from 'lucide-react';
import { 
  AreaChart, 
  Area, 
  XAxis, 
  YAxis, 
  CartesianGrid, 
  Tooltip, 
  ResponsiveContainer,
  BarChart,
  Bar,
  Cell,
  PieChart,
  Pie
} from 'recharts';
import { motion } from 'framer-motion';
import './index.css';

// Mock Data
const hourlyData = [
  { time: '00:00', requests: 1200, latency: 120 },
  { time: '02:00', requests: 900, latency: 110 },
  { time: '04:00', requests: 600, latency: 90 },
  { time: '06:00', requests: 1500, latency: 140 },
  { time: '08:00', requests: 3800, latency: 210 },
  { time: '10:00', requests: 5200, latency: 280 },
  { time: '12:00', requests: 4800, latency: 250 },
  { time: '14:00', requests: 5500, latency: 290 },
  { time: '16:00', requests: 4200, latency: 230 },
  { time: '18:00', requests: 3100, latency: 180 },
  { time: '20:00', requests: 2400, latency: 150 },
  { time: '22:00', requests: 1800, latency: 130 },
];

const agentStats = [
  { name: 'Translator', success: 98.5, error: 1.5, requests: 15420 },
  { name: 'Code Assistant', success: 94.2, error: 5.8, requests: 8340 },
  { name: 'Data Analyst', success: 91.8, error: 8.2, requests: 5210 },
  { name: 'Support Bot', success: 99.1, error: 0.9, requests: 21050 },
];

const distributionData = [
  { name: 'Success', value: 96.8, color: '#10b981' },
  { name: 'Error', value: 3.2, color: '#ef4444' },
];

const CustomTooltip = ({ active, payload, label }: any) => {
  if (active && payload && payload.length) {
    return (
      <div className="custom-tooltip">
        <p className="tooltip-label">{label}</p>
        {payload.map((entry: any, index: number) => (
          <div key={index} className="tooltip-value" style={{ color: entry.color }}>
            <span>{entry.name}:</span>
            <span>{entry.value}{entry.name === 'latency' ? 'ms' : ''}</span>
          </div>
        ))}
      </div>
    );
  }
  return null;
};

const MetricCard = ({ title, value, unit, icon: Icon, trend, trendValue, type, delay }: any) => (
  <motion.div 
    initial={{ opacity: 0, y: 20 }}
    animate={{ opacity: 1, y: 0 }}
    transition={{ duration: 0.5, delay }}
    className={`glass-panel metric-card ${type}`}
  >
    <div className="metric-header">
      <span>{title}</span>
      <div className="metric-icon">
        <Icon size={20} className={`text-${type}`} style={{ color: `var(--${type || 'accent-primary'})` }} />
      </div>
    </div>
    <div className="metric-value">
      {value} <span className="metric-unit">{unit}</span>
    </div>
    <div className={`metric-trend ${trend}`}>
      {trend === 'up' ? <TrendingUp size={16} /> : <TrendingDown size={16} />}
      <span>{trendValue} vs last 24h</span>
    </div>
  </motion.div>
);

function App() {
  const [activeTab, setActiveTab] = useState('overview');
  const [isLoaded, setIsLoaded] = useState(false);

  useEffect(() => {
    setIsLoaded(true);
  }, []);

  if (!isLoaded) return null;

  return (
    <div className="dashboard-container">
      <motion.header 
        initial={{ opacity: 0, y: -20 }}
        animate={{ opacity: 1, y: 0 }}
        className="dashboard-header"
      >
        <div className="dashboard-title">
          <Activity size={36} className="dashboard-title-icon" />
          <span>Nasiko Titan Metrics</span>
        </div>
        
        <div className="nav-bar">
          <button className={`nav-item ${activeTab === 'overview' ? 'active' : ''}`} onClick={() => setActiveTab('overview')}>
            Overview
          </button>
          <button className={`nav-item ${activeTab === 'agents' ? 'active' : ''}`} onClick={() => setActiveTab('agents')}>
            Agents
          </button>
          <button className={`nav-item ${activeTab === 'system' ? 'active' : ''}`} onClick={() => setActiveTab('system')}>
            System
          </button>
        </div>

        <div className="live-badge">
          <div className="live-dot"></div>
          Live Data
        </div>
      </motion.header>

      <div className="metrics-grid">
        <MetricCard 
          title="Average Response Time" 
          value="184" 
          unit="ms" 
          icon={Clock} 
          trend="down" 
          trendValue="-12ms" 
          type="accent-primary"
          delay={0.1}
        />
        <MetricCard 
          title="Total Success Rate" 
          value="96.8" 
          unit="%" 
          icon={CheckCircle2} 
          trend="up" 
          trendValue="+0.4%" 
          type="success"
          delay={0.2}
        />
        <MetricCard 
          title="Error Count (24h)" 
          value="1,602" 
          unit="reqs" 
          icon={XCircle} 
          trend="down" 
          trendValue="-5.2%" 
          type="error"
          delay={0.3}
        />
        <MetricCard 
          title="System Uptime" 
          value="99.99" 
          unit="%" 
          icon={Server} 
          trend="up" 
          trendValue="+0.01%" 
          type="accent-secondary"
          delay={0.4}
        />
      </div>

      <div className="charts-grid">
        <motion.div 
          initial={{ opacity: 0, scale: 0.95 }}
          animate={{ opacity: 1, scale: 1 }}
          transition={{ duration: 0.5, delay: 0.5 }}
          className="glass-panel chart-card"
        >
          <div className="chart-header">
            <h3 className="chart-title">Request Traffic & Latency</h3>
            <p className="chart-subtitle">Volume and response time over the last 24 hours</p>
          </div>
          <div className="chart-container">
            <ResponsiveContainer width="100%" height="100%">
              <AreaChart data={hourlyData} margin={{ top: 10, right: 30, left: 0, bottom: 0 }}>
                <defs>
                  <linearGradient id="colorReqs" x1="0" y1="0" x2="0" y2="1">
                    <stop offset="5%" stopColor="#6366f1" stopOpacity={0.4}/>
                    <stop offset="95%" stopColor="#6366f1" stopOpacity={0}/>
                  </linearGradient>
                  <linearGradient id="colorLat" x1="0" y1="0" x2="0" y2="1">
                    <stop offset="5%" stopColor="#8b5cf6" stopOpacity={0.4}/>
                    <stop offset="95%" stopColor="#8b5cf6" stopOpacity={0}/>
                  </linearGradient>
                </defs>
                <CartesianGrid strokeDasharray="3 3" stroke="rgba(255,255,255,0.05)" vertical={false} />
                <XAxis dataKey="time" stroke="#606075" tick={{fill: '#a0a0b0'}} axisLine={false} tickLine={false} />
                <YAxis yAxisId="left" stroke="#606075" tick={{fill: '#a0a0b0'}} axisLine={false} tickLine={false} />
                <YAxis yAxisId="right" orientation="right" stroke="#606075" tick={{fill: '#a0a0b0'}} axisLine={false} tickLine={false} />
                <Tooltip content={<CustomTooltip />} />
                <Area yAxisId="left" type="monotone" dataKey="requests" name="Requests" stroke="#6366f1" strokeWidth={3} fillOpacity={1} fill="url(#colorReqs)" />
                <Area yAxisId="right" type="monotone" dataKey="latency" name="Latency (ms)" stroke="#8b5cf6" strokeWidth={3} fillOpacity={1} fill="url(#colorLat)" />
              </AreaChart>
            </ResponsiveContainer>
          </div>
        </motion.div>

        <motion.div 
          initial={{ opacity: 0, scale: 0.95 }}
          animate={{ opacity: 1, scale: 1 }}
          transition={{ duration: 0.5, delay: 0.6 }}
          className="glass-panel chart-card"
        >
          <div className="chart-header">
            <h3 className="chart-title">Global Status Distribution</h3>
            <p className="chart-subtitle">Overall success vs error ratio</p>
          </div>
          <div className="chart-container" style={{ display: 'flex', flexDirection: 'column', alignItems: 'center', justifyContent: 'center' }}>
            <ResponsiveContainer width="100%" height="80%">
              <PieChart>
                <Pie
                  data={distributionData}
                  cx="50%"
                  cy="50%"
                  innerRadius={80}
                  outerRadius={110}
                  paddingAngle={5}
                  dataKey="value"
                  stroke="none"
                >
                  {distributionData.map((entry, index) => (
                    <Cell key={`cell-${index}`} fill={entry.color} />
                  ))}
                </Pie>
                <Tooltip content={<CustomTooltip />} />
              </PieChart>
            </ResponsiveContainer>
            <div style={{ display: 'flex', gap: '2rem', marginTop: '1rem' }}>
              {distributionData.map(item => (
                <div key={item.name} style={{ display: 'flex', alignItems: 'center', gap: '0.5rem' }}>
                  <div style={{ width: 12, height: 12, borderRadius: '50%', backgroundColor: item.color }}></div>
                  <span style={{ color: 'var(--text-secondary)', fontWeight: 500 }}>{item.name} ({item.value}%)</span>
                </div>
              ))}
            </div>
          </div>
        </motion.div>

        <motion.div 
          initial={{ opacity: 0, y: 20 }}
          animate={{ opacity: 1, y: 0 }}
          transition={{ duration: 0.5, delay: 0.7 }}
          className="glass-panel chart-card"
          style={{ gridColumn: '1 / -1' }}
        >
          <div className="chart-header">
            <h3 className="chart-title">Agent Performance Breakdown</h3>
            <p className="chart-subtitle">Success rate and total requests by agent</p>
          </div>
          <div className="chart-container" style={{ height: 300 }}>
            <ResponsiveContainer width="100%" height="100%">
              <BarChart data={agentStats} margin={{ top: 20, right: 30, left: 0, bottom: 5 }} barSize={40}>
                <CartesianGrid strokeDasharray="3 3" stroke="rgba(255,255,255,0.05)" vertical={false} />
                <XAxis dataKey="name" stroke="#606075" tick={{fill: '#a0a0b0'}} axisLine={false} tickLine={false} />
                <YAxis yAxisId="left" stroke="#606075" tick={{fill: '#a0a0b0'}} axisLine={false} tickLine={false} domain={[80, 100]} />
                <YAxis yAxisId="right" orientation="right" stroke="#606075" tick={{fill: '#a0a0b0'}} axisLine={false} tickLine={false} />
                <Tooltip content={<CustomTooltip />} cursor={{fill: 'rgba(255,255,255,0.05)'}} />
                <Bar yAxisId="left" dataKey="success" name="Success Rate %" radius={[4, 4, 0, 0]}>
                  {agentStats.map((entry, index) => (
                    <Cell key={`cell-${index}`} fill={entry.success > 95 ? '#10b981' : '#f59e0b'} />
                  ))}
                </Bar>
                <Bar yAxisId="right" dataKey="requests" name="Total Requests" fill="#6366f1" radius={[4, 4, 0, 0]} opacity={0.3} />
              </BarChart>
            </ResponsiveContainer>
          </div>
        </motion.div>

      </div>
    </div>
  );
}

export default App;
