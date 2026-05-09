const express = require("express");
const http = require("http");
const cors = require("cors");
const morgan = require("morgan");
const { Server: SocketIOServer } = require("socket.io");
const Redis = require("ioredis");
const config = require("./config");

// --- Express App ---
const app = express();
const server = http.createServer(app);

// --- Socket.io ---
const io = new SocketIOServer(server, {
  cors: {
    origin: [config.dashboardUrl, "http://localhost:5173", "http://localhost:3000"],
    methods: ["GET", "POST"],
  },
});

// --- Redis ---
const redis = new Redis(config.redisUrl, {
  maxRetriesPerRequest: null, // Required by BullMQ
  retryStrategy: (times) => Math.min(times * 200, 5000),
});

redis.on("connect", () => console.log("✅ Redis connected"));
redis.on("error", (err) => console.error("❌ Redis error:", err.message));

// --- Middleware ---
app.use(cors({
  origin: [config.dashboardUrl, "http://localhost:5173", "http://localhost:3000"],
  credentials: true,
}));
app.use(express.json());
app.use(morgan("dev"));

// --- Initialize core services ---
const RateLimiter = require("./limiter/rateLimiter");
const QueueManager = require("./queue/queueManager");
const StatsCollector = require("./metrics/statsCollector");

const rateLimiter = new RateLimiter(redis);
const queueManager = new QueueManager(redis, rateLimiter);
const statsCollector = new StatsCollector(redis);

// Make shared instances available to routes
app.set("redis", redis);
app.set("io", io);
app.set("queueManager", queueManager);
app.set("statsCollector", statsCollector);

// --- Health Endpoint ---
app.get("/api/health", async (req, res) => {
  let redisStatus = "disconnected";
  try {
    await redis.ping();
    redisStatus = "connected";
  } catch {
    redisStatus = "error";
  }

  res.json({
    status: "ok",
    service: "nasiko-request-layer",
    uptime: process.uptime(),
    timestamp: new Date().toISOString(),
    redis: redisStatus,
  });
});

// --- Route Imports ---
const processRoutes = require("./routes/processRoutes");
const cacheRoutes = require("./routes/cacheRoutes");
const limiterRoutes = require("./routes/limiterRoutes");
const queueRoutes = require("./routes/queueRoutes");
const statsRoutes = require("./routes/statsRoutes");
const agentRoutes = require("./routes/agentRoutes");

app.use("/api", processRoutes);
app.use("/api/cache", cacheRoutes);
app.use("/api/limits", limiterRoutes);
app.use("/api/queue", queueRoutes);
app.use("/api/stats", statsRoutes);
app.use("/api/agents", agentRoutes);

// --- Socket.io Connection ---
io.on("connection", (socket) => {
  console.log(`📡 Dashboard connected: ${socket.id}`);
  socket.on("disconnect", () => {
    console.log(`📡 Dashboard disconnected: ${socket.id}`);
  });
});

// --- Periodic Stats Broadcast (every 2s for real-time dashboard) ---
setInterval(async () => {
  if (io.engine.clientsCount > 0) {
    try {
      const globalStats = await statsCollector.getGlobalStats();
      const queueStatus = queueManager.getStatus();
      const recentRequests = await statsCollector.getRecentRequests(10);
      io.emit("stats:update", { globalStats, queueStatus, recentRequests });
    } catch (err) {
      // Silently handle — dashboard may not be connected
    }
  }
}, 2000);

// --- Start Server ---
server.listen(config.port, () => {
  console.log(`
  ╔══════════════════════════════════════════════════╗
  ║   🛡️  Nasiko Request Layer                       ║
  ║   Adaptive Traffic Control for Agent Orchestration║
  ║                                                  ║
  ║   Server:    http://localhost:${config.port}              ║
  ║   Health:    http://localhost:${config.port}/api/health    ║
  ║   Redis:     ${config.redisUrl}            ║
  ╚══════════════════════════════════════════════════╝
  `);
});

module.exports = { app, server, io, redis };
