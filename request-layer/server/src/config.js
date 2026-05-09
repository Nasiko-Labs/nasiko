require("dotenv").config();

const config = {
  port: parseInt(process.env.PORT, 10) || 3000,

  // Redis
  redisUrl: process.env.REDIS_URL || "redis://localhost:6379",

  // Kong Gateway (real Nasiko agents)
  kongGatewayUrl: process.env.KONG_GATEWAY_URL || "http://localhost:9100",

  // Nasiko Backend API
  nasikoBackendUrl: process.env.NASIKO_BACKEND_URL || "http://localhost:8000/api/v1",

  // Cache
  cacheDefaultTTL: parseInt(process.env.CACHE_DEFAULT_TTL, 10) || 300,

  // Rate Limiter
  rateLimitMaxTokens: parseInt(process.env.RATE_LIMIT_MAX_TOKENS, 10) || 10,
  rateLimitRefillRate: parseInt(process.env.RATE_LIMIT_REFILL_RATE, 10) || 2,
  rateLimitMaxQueueSize: parseInt(process.env.RATE_LIMIT_MAX_QUEUE_SIZE, 10) || 20,
  rateLimitMaxWaitTime: parseInt(process.env.RATE_LIMIT_MAX_WAIT_TIME, 10) || 30000,

  // Dashboard
  dashboardUrl: process.env.DASHBOARD_URL || "http://localhost:5173",
};

module.exports = config;
