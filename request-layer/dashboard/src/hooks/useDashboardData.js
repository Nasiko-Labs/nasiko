import { useEffect, useState, useCallback } from 'react';
import { io } from 'socket.io-client';
import axios from 'axios';

const SOCKET_URL = 'http://localhost:3000';
const API_URL = 'http://localhost:3000/api';

export function useDashboardData() {
  const [socket, setSocket] = useState(null);
  const [globalStats, setGlobalStats] = useState(null);
  const [queueStatus, setQueueStatus] = useState({});
  const [recentRequests, setRecentRequests] = useState([]);
  const [agentStats, setAgentStats] = useState({});
  const [timeSeries, setTimeSeries] = useState([]);
  const [limits, setLimits] = useState({});
  
  // Initial data fetch
  const fetchInitialData = useCallback(async () => {
    try {
      const [globalRes, queueRes, feedRes, agentsRes, historyRes, limitsRes] = await Promise.all([
        axios.get(`${API_URL}/stats`),
        axios.get(`${API_URL}/queue/status`),
        axios.get(`${API_URL}/stats/feed`),
        axios.get(`${API_URL}/stats/agents`),
        axios.get(`${API_URL}/stats/history?duration=1800000`), // 30 mins
        axios.get(`${API_URL}/limits`),
      ]);

      setGlobalStats(globalRes.data);
      setQueueStatus(queueRes.data.queues || {});
      setRecentRequests(feedRes.data.requests || []);
      setAgentStats(agentsRes.data || {});
      setTimeSeries(historyRes.data.series || []);
      setLimits(limitsRes.data.configs || {});
    } catch (err) {
      console.error("Failed to fetch initial data", err);
    }
  }, []);

  useEffect(() => {
    fetchInitialData();

    const newSocket = io(SOCKET_URL);
    setSocket(newSocket);

    newSocket.on('connect', () => console.log('Socket connected'));
    
    // Periodic full update (every 2s from backend)
    newSocket.on('stats:update', (data) => {
      if (data.globalStats) setGlobalStats(data.globalStats);
      if (data.queueStatus) setQueueStatus(data.queueStatus);
      if (data.recentRequests) setRecentRequests(data.recentRequests);
    });

    // We can also listen to individual events to make it feel more real-time
    newSocket.on('request:completed', (req) => {
      setRecentRequests(prev => [req, ...prev].slice(0, 50));
    });

    newSocket.on('limits:updated', (data) => {
       setLimits(prev => ({...prev, [data.agent]: data.config}));
    });

    const interval = setInterval(async () => {
       // Refresh history and agent stats every 10s
       try {
         const [historyRes, agentsRes] = await Promise.all([
           axios.get(`${API_URL}/stats/history?duration=1800000`),
           axios.get(`${API_URL}/stats/agents`)
         ]);
         setTimeSeries(historyRes.data.series || []);
         setAgentStats(agentsRes.data || {});
       } catch (e) {}
    }, 10000);

    return () => {
      newSocket.disconnect();
      clearInterval(interval);
    };
  }, [fetchInitialData]);

  const flushCache = async () => {
    try {
      await axios.delete(`${API_URL}/cache/flush`);
      await fetchInitialData();
    } catch (e) {
      console.error(e);
    }
  };

  const updateLimit = async (agent, config) => {
    try {
      await axios.put(`${API_URL}/limits/${agent}`, config);
    } catch (e) {
      console.error(e);
    }
  };

  return {
    globalStats,
    queueStatus,
    recentRequests,
    agentStats,
    timeSeries,
    limits,
    flushCache,
    updateLimit
  };
}
