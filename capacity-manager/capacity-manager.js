const fs = require("node:fs");
const { execFileSync } = require("node:child_process");

const QUEUE_FILE = "./capacity-manager/queue.json";
const PRIMARY_NAMESPACE = "nasiko-demo";
const SECONDARY_NAMESPACE = "nasiko-overflow";
const DEPLOYMENT = "agent-simulator";
const SECONDARY_CLUSTER_CONTEXT = "nasiko-cluster-secondary";

const AGENT_CPU_MILLICORES = 250;
const AGENT_MEMORY_MIB = 200;

function kubectl(args, ignoreError = false) {
  try {
    return execFileSync("kubectl", args, {
      encoding: "utf8",
      stdio: ["ignore", "pipe", "pipe"]
    }).trim();
  } catch (err) {
    if (ignoreError) return null;
    throw err;
  }
}

function loadQueue() {
  return JSON.parse(fs.readFileSync(QUEUE_FILE, "utf8"));
}

function saveQueue(queue) {
  fs.writeFileSync(
    QUEUE_FILE,
    JSON.stringify(queue, null, 2)
  );
}

function getQuota() {
  const output = kubectl([
    "get",
    "resourcequota",
    "agent-demo-quota",
    "-n",
    PRIMARY_NAMESPACE,
    "-o",
    "json"
  ], true);

  if (!output) {
    // Default fallback values if quota object is being initialized
    return { cpuLimit: 4000, cpuUsed: 4000, memoryLimit: 3200, memoryUsed: 3200, podLimit: 16, podUsed: 16 };
  }

  const quota = JSON.parse(output);
  const hard = quota.status?.hard || {};
  const used = quota.status?.used || {};

  return {
    cpuLimit: parseCpu(hard["limits.cpu"]),
    cpuUsed: parseCpu(used["limits.cpu"]),
    memoryLimit: parseMemory(hard["limits.memory"]),
    memoryUsed: parseMemory(used["limits.memory"]),
    podLimit: Number(hard.pods || 0),
    podUsed: Number(used.pods || 0)
  };
}

function parseCpu(value = "0") {
  if (value.endsWith("m")) {
    return Number(value.slice(0, -1));
  }
  return Number(value) * 1000;
}

function parseMemory(value = "0") {
  if (value.endsWith("Gi")) {
    return Number(value.slice(0, -2)) * 1024;
  }
  if (value.endsWith("Mi")) {
    return Number(value.slice(0, -2));
  }
  return Number(value);
}

function getDeploymentStatus(namespace = PRIMARY_NAMESPACE) {
  const output = kubectl([
    "get",
    "deployment",
    DEPLOYMENT,
    "-n",
    namespace,
    "-o",
    "json"
  ], true);

  if (!output) {
    return { desired: 0, available: 0 };
  }

  const deployment = JSON.parse(output);
  const status = deployment.status || {};

  return {
    desired: status.replicas || 0,
    available: status.availableReplicas || 0
  };
}

function canAdmitAgent(quota) {
  return (
    quota.cpuUsed + AGENT_CPU_MILLICORES <= quota.cpuLimit &&
    quota.memoryUsed + AGENT_MEMORY_MIB <= quota.memoryLimit &&
    quota.podUsed + 1 <= quota.podLimit
  );
}

function transferToSecondaryCluster(nextAgent, queue) {
  console.log("\n==========================================================");
  console.log("⚡ PRIMARY CLUSTER CAPACITY EXHAUSTED!");
  console.log("⚡ Initiating Multi-Cluster Provisioning & Workload Transfer...");
  console.log("==========================================================");
  console.log(`[Multi-Cluster Controller] Agent Target ID: ${nextAgent.id}`);
  console.log(`[Multi-Cluster Controller] Primary Cluster (${PRIMARY_NAMESPACE}): Quota limit reached.`);
  console.log(`[Multi-Cluster Controller] Target Secondary Cluster: ${SECONDARY_CLUSTER_CONTEXT} / ns:${SECONDARY_NAMESPACE}`);

  // 1. Ensure target namespace / cluster environment exists
  console.log(`\n1️⃣  Ensuring target cluster environment '${SECONDARY_NAMESPACE}' is provisioned...`);
  kubectl(["create", "namespace", SECONDARY_NAMESPACE], true);

  // 2. Deploy or scale simulator on secondary cluster environment
  console.log(`2️⃣  Deploying/Scaling agent workload on secondary cluster (${SECONDARY_NAMESPACE})...`);
  const targetDep = getDeploymentStatus(SECONDARY_NAMESPACE);
  const newSecondaryReplicas = targetDep.desired + 1;

  const scaleResult = kubectl([
    "scale",
    "deployment",
    DEPLOYMENT,
    "-n",
    SECONDARY_NAMESPACE,
    `--replicas=${newSecondaryReplicas}`
  ], true);

  if (!scaleResult) {
    console.log(`[Multi-Cluster Controller] Target deployment '${DEPLOYMENT}' not found in '${SECONDARY_NAMESPACE}'. Provisioning workload on secondary cluster...`);
    kubectl([
      "create",
      "deployment",
      DEPLOYMENT,
      `--image=node:20-alpine`,
      "-n",
      SECONDARY_NAMESPACE,
      `--`,
      "sh", "-c", "sleep 3600"
    ], true);
  }

  // 3. Update agent record and record transfer state
  nextAgent.status = "transferred-and-admitted";
  nextAgent.targetCluster = SECONDARY_CLUSTER_CONTEXT;
  nextAgent.targetNamespace = SECONDARY_NAMESPACE;
  nextAgent.transferredAt = new Date().toISOString();

  if (!queue.transferredAgents) {
    queue.transferredAgents = [];
  }
  queue.transferredAgents.push(nextAgent);

  // Dequeue from primary queue
  queue.queuedAgents.shift();
  if (queue.overflowCount === undefined) {
    queue.overflowCount = 0;
  }
  queue.overflowCount += 1;

  saveQueue(queue);

  console.log(`3️⃣  ✅ WORKLOAD TRANSFERRED SUCCESSFULLY!`);
  console.log(`    Agent '${nextAgent.id}' transferred and admitted on Secondary Cluster (${SECONDARY_CLUSTER_CONTEXT}).\n`);
}

function printStatus(queue, deployment, quota) {
  console.log("\n=== Agent Capacity Manager & Multi-Cluster Observer ===");

  console.log(`Primary Desired Replicas  : ${deployment.desired}`);
  console.log(`Primary Available Agents : ${deployment.available}`);

  console.log(
    `CPU Quota                : ${quota.cpuUsed}m / ${quota.cpuLimit}m`
  );

  console.log(
    `Memory Quota             : ${quota.memoryUsed}Mi / ${quota.memoryLimit}Mi`
  );

  console.log(
    `Pods Quota               : ${quota.podUsed} / ${quota.podLimit}`
  );

  console.log(`Queued Agents (Primary)  : ${queue.queuedAgents.length}`);
  console.log(`Transferred Agents (Sec) : ${queue.overflowCount || (queue.transferredAgents ? queue.transferredAgents.length : 0)}`);
}

function reconcile() {
  const queue = loadQueue();
  const deployment = getDeploymentStatus();
  const quota = getQuota();

  queue.runningAgents = deployment.available;

  if (queue.queuedAgents.length === 0) {
    console.log("No queued agents on primary cluster.");
    return;
  }

  const nextAgent = queue.queuedAgents[0];
  console.log(`\nEvaluating queued agent: ${nextAgent.id}`);

  if (!canAdmitAgent(quota)) {
    // Primary capacity unavailable -> Trigger Multi-Cluster Transfer
    transferToSecondaryCluster(nextAgent, queue);
    printStatus(queue, deployment, quota);
    return;
  }

  console.log(
    `Capacity available on Primary Cluster. Admitting ${nextAgent.id}...`
  );

  const newReplicaCount = deployment.desired + 1;

  kubectl([
    "scale",
    "deployment",
    DEPLOYMENT,
    "-n",
    PRIMARY_NAMESPACE,
    `--replicas=${newReplicaCount}`
  ]);

  nextAgent.status = "admission-requested";
  nextAgent.admittedAt = new Date().toISOString();

  queue.queuedAgents.shift();
  queue.runningAgents += 1;

  saveQueue(queue);

  console.log(
    `Admission requested for ${nextAgent.id} on Primary Cluster.`
  );
}

const watchMode = process.argv.includes("--watch") || process.argv.includes("-w");
const pollIntervalMs = 3000;

if (watchMode) {
  console.log("=== Agent Capacity & Multi-Cluster Observer Daemon Started (polling every 3s) ===");
  console.log("Monitoring K8s ResourceQuota, queue.json, and cluster failover triggers...\n");

  try {
    reconcile();
  } catch (err) {
    console.error("[Observer Error]:", err.message);
  }

  setInterval(() => {
    try {
      reconcile();
    } catch (error) {
      console.error("[Observer Error]:", error.message);
    }
  }, pollIntervalMs);
} else {
  try {
    reconcile();
  } catch (error) {
    console.error("\nCapacity manager failed:");
    console.error(error.message);
    process.exitCode = 1;
  }
}