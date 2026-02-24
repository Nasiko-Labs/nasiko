# Kubernetes Deployment Troubleshooting Guide

This guide covers common issues encountered when deploying Nasiko on Kubernetes (DigitalOcean, AWS, or other cloud providers) and their solutions.

## Table of Contents

1. [Registry Authentication Issues](#registry-authentication-issues)
2. [BuildKit Job Failures](#buildkit-job-failures)
3. [Pod Failures and ImagePullBackOff](#pod-failures-and-imagepullbackoff)
4. [Secret Management](#secret-management)
5. [Network and Service Discovery](#network-and-service-discovery)
   - [Agent Chat Returns 404 Not Found](#problem-agent-chat-returns-404-not-found)
   - [Services Can't Reach BuildKit](#problem-services-cant-reach-buildkit)
6. [Resource Issues](#resource-issues)
7. [General Debugging Commands](#general-debugging-commands)

---

## Registry Authentication Issues

### Problem: Registry Name Mismatch (DigitalOcean)

**Symptoms:**
```
❌ Registry name mismatch!
   Requested registry: 'nasiko-images'
   Your actual registry: 'my-registry'

   DigitalOcean allows only ONE registry per account.
```

**Root Cause:**
You're trying to use a registry name that doesn't match your existing DigitalOcean registry. DigitalOcean accounts can only have ONE registry.

**Solution:**

#### Option 1: Use Your Existing Registry (Recommended)
```bash
# Find your actual registry name
doctl registry get

# Use it in bootstrap command
uv run cli/main.py setup bootstrap \
  --kubeconfig /path/to/kubeconfig.yaml \
  --registry-type cloud \
  --cloud-reg-name <your-actual-registry-name>
```

#### Option 2: Delete and Recreate
```bash
# Delete existing registry (WARNING: This deletes all images!)
doctl registry delete --force

# Re-run bootstrap with desired name
uv run cli/main.py setup bootstrap \
  --kubeconfig /path/to/kubeconfig.yaml \
  --registry-type cloud \
  --cloud-reg-name nasiko-images
```

---

### Problem: BuildKit Job Fails with 401 Unauthorized

**Symptoms:**
```
ERROR: failed to push registry.digitalocean.com/nasiko-images/<agent-name>:v<timestamp>:
failed to authorize: failed to fetch oauth token: unexpected status from GET request to
https://api.digitalocean.com/v2/registry/auth?scope=repository%3A...: 401 Unauthorized
```

**Root Causes:**
1. **Wrong User Credentials**: Secret contains credentials for a different user than the one authorized with `doctl`
2. **Read-Only Token**: Token was generated without `--read-write` flag
3. **Expired Token**: Token has exceeded its expiry time
4. **Invalid Token**: Token was revoked or is malformed
5. **Registry Name Mismatch**: Trying to use wrong registry name (see above)

**Solution:**

#### Step 1: Verify Current Registry
```bash
# Check which registry is configured
doctl registry get
```

#### Step 2: Regenerate Credentials with Read-Write Access
```bash
# Generate fresh credentials with 1-year expiry
doctl registry docker-config --read-write --expiry-seconds 31536000
```

#### Step 3: Update Kubernetes Secrets
```bash
# Set your kubeconfig
export KUBECONFIG=/path/to/your/kubeconfig.yaml

# Delete old secrets
kubectl delete secret agent-registry-credentials -n nasiko-agents
kubectl delete secret regcred -n nasiko

# Create new secrets with fresh credentials
doctl registry docker-config --read-write --expiry-seconds 31536000 | \
  kubectl create secret generic agent-registry-credentials \
  --from-file=.dockerconfigjson=/dev/stdin \
  --type=kubernetes.io/dockerconfigjson \
  -n nasiko-agents

doctl registry docker-config --read-write --expiry-seconds 31536000 | \
  kubectl create secret generic regcred \
  --from-file=.dockerconfigjson=/dev/stdin \
  --type=kubernetes.io/dockerconfigjson \
  -n nasiko
```

#### Step 4: Verify Secret Contents
```bash
# Check the secret
kubectl get secret agent-registry-credentials -n nasiko-agents -o jsonpath='{.data.\.dockerconfigjson}' | base64 -d | python3 -m json.tool

# Decode and verify username matches your doctl user
kubectl get secret agent-registry-credentials -n nasiko-agents -o jsonpath='{.data.\.dockerconfigjson}' | \
  base64 -d | jq -r '.auths."registry.digitalocean.com".auth' | base64 -d
```

#### Step 5: Clean Up Failed Jobs
```bash
# List failed jobs
kubectl get jobs -n nasiko-agents

# Delete failed job to allow retry
kubectl delete job <job-name> -n nasiko-agents

# Or delete all failed jobs
kubectl delete jobs -n nasiko-agents --field-selector status.successful=0
```

**Prevention:**
- Always use `--read-write` flag when generating registry tokens
- Set long expiry times (1 year) to avoid frequent token refreshes
- Document which user account owns the registry
- Use automation/CronJobs to refresh tokens before expiry (for AWS ECR especially)

---

## BuildKit Job Failures

### Problem: BuildKit Job Stays in Pending State

**Symptoms:**
```bash
kubectl get jobs -n nasiko-agents
# NAME                              COMPLETIONS   DURATION   AGE
# job-a2a-github-agent-1766140572   0/1           5m         5m
```

**Diagnosis:**
```bash
# Check pod status
kubectl get pods -n nasiko-agents -l job-name=<job-name>

# Describe pod to see events
kubectl describe pod <pod-name> -n nasiko-agents
```

**Common Causes:**

#### 1. BuildKit Service Not Running
```bash
# Check buildkit pod
kubectl get pods -n buildkit

# If not running, check deployment
kubectl get deployment -n buildkit
```

**Solution:**
```bash
# Redeploy buildkit
uv run cli/main.py setup buildkit deploy
```

#### 2. Resource Constraints
Check if nodes have enough resources:
```bash
kubectl describe nodes
kubectl top nodes  # Requires metrics-server
```

**Solution:**
- Scale up cluster nodes
- Reduce resource requests in job spec

#### 3. ImagePullBackOff on Init Container
```bash
kubectl logs <pod-name> -n nasiko-agents -c download-agent
```

**Solution:**
- Check if git repository is accessible
- Verify network connectivity from cluster

---

### Problem: Build Job Fails During Image Build

**Symptoms:**
```bash
kubectl logs <pod-name> -n nasiko-agents -c buildkit-client
# Error: failed to solve: <build error>
```

**Common Build Errors:**

#### 1. Missing Dockerfile
```
ERROR: failed to load Dockerfile: no such file or directory
```

**Solution:**
- Ensure agent repository has a `Dockerfile` in the root
- Check git clone completed successfully in init container

#### 2. Build Context Too Large
```
ERROR: context too large
```

**Solution:**
- Add `.dockerignore` file to exclude unnecessary files
- Check for large files in repository

#### 3. Network Issues During Build
```
ERROR: failed to fetch <package>: connection timeout
```

**Solution:**
- Check cluster DNS resolution
- Verify outbound internet connectivity
- Check if registry mirrors are needed

---

## Pod Failures and ImagePullBackOff

### Problem: Agent Pod Can't Pull Image

**Symptoms:**
```bash
kubectl get pods -n nasiko-agents
# NAME                    READY   STATUS             RESTARTS   AGE
# agent-abc123-xxx        0/1     ImagePullBackOff   0          2m
```

**Diagnosis:**
```bash
kubectl describe pod <pod-name> -n nasiko-agents
# Look for: "Failed to pull image" or "unauthorized: authentication required"
```

**Solutions:**

#### 1. Missing ImagePullSecret
```bash
# Check if deployment has imagePullSecrets
kubectl get deployment <deployment-name> -n nasiko-agents -o yaml | grep imagePullSecrets -A 2

# If missing, patch deployment
kubectl patch deployment <deployment-name> -n nasiko-agents -p '{"spec":{"template":{"spec":{"imagePullSecrets":[{"name":"agent-registry-credentials"}]}}}}'
```

#### 2. Wrong Image Name/Tag
```bash
# Verify image exists in registry
doctl registry repository list-tags nasiko-images/<agent-name>
```

#### 3. Registry Credentials Issue
Follow steps in [Registry Authentication Issues](#registry-authentication-issues)

---

## Secret Management

### Checking Secret Contents

```bash
# List secrets in namespace
kubectl get secrets -n nasiko-agents

# View secret details (base64 encoded)
kubectl get secret <secret-name> -n nasiko-agents -o yaml

# Decode docker config secret
kubectl get secret agent-registry-credentials -n nasiko-agents \
  -o jsonpath='{.data.\.dockerconfigjson}' | base64 -d | python3 -m json.tool

# Decode registry auth
kubectl get secret agent-registry-credentials -n nasiko-agents \
  -o jsonpath='{.data.\.dockerconfigjson}' | base64 -d | \
  jq -r '.auths."registry.digitalocean.com".auth' | base64 -d
```

### Updating Secrets

```bash
# Method 1: Delete and recreate
kubectl delete secret <secret-name> -n <namespace>
kubectl create secret <type> <secret-name> -n <namespace> <options>

# Method 2: Patch existing secret
kubectl patch secret <secret-name> -n <namespace> -p '{"data":{"key":"value"}}'
```

---

## Network and Service Discovery

### Problem: Agent Chat Returns 404 Not Found

**Symptoms:**
```
POST http://<gateway-ip>/agents/<agent-name> 404 (Not Found)
Resource not found
```

When trying to chat with an agent through the web UI, requests to the agent endpoint return 404 errors.

**Root Cause:**
Mismatch between Kong route paths and the URLs the frontend uses:
- Frontend expects: `http://<gateway-ip>/agents/<agent-name>` (e.g., `/agents/a2a-github-agent`)
- Kong route was: `http://<gateway-ip>/agents/<deployment-name>` (e.g., `/agents/agent-a2a-github-agent-1766379932`)

The issue occurs because:
1. Agent deployments include a timestamp suffix in their Kubernetes deployment names
2. The service registry was creating Kong routes using the full deployment name
3. The build worker was generating agent URLs using just the agent name (without timestamp)

**Diagnosis:**

#### Step 1: Check Agent Deployment Status
```bash
# Check if agent is deployed
export KUBECONFIG=/path/to/kubeconfig.yaml
kubectl get deployments -n nasiko-agents
kubectl get services -n nasiko-agents

# Expected output should show deployment like: agent-<name>-<timestamp>
```

#### Step 2: Verify Kong Routes
```bash
# Check service registry logs
kubectl logs -n nasiko -l app=k8s-service-registry --tail=20

# Should show routes being registered like:
# "Discovered agent service: agent-<name>-<timestamp> at <host>:<port> (route: /agents/<agent-name>)"
```

#### Step 3: Check Service Labels
```bash
# Verify the service has the agent-name label
kubectl get service -n nasiko-agents <service-name> -o yaml | grep -A 5 "labels:"

# Should show:
#   labels:
#     agent-name: <agent-name>  # This should match the agent name without timestamp
#     app: <deployment-name>
```

**Solution:**

This issue was fixed in commits that updated:
1. `app/service/k8s_service.py` - Added `agent-name` label to Kubernetes services
2. `agent-gateway/registry/registry.py` - Updated service discovery to use the label for Kong routing

If you're experiencing this issue, follow these steps:

#### Step 1: Rebuild and Deploy Updated Components
```bash
# Rebuild service registry
cd /path/to/nasiko/agent-gateway/registry
docker build -t <your-dockerhub-user>/nasiko-service-registry:latest .
docker push <your-dockerhub-user>/nasiko-service-registry:latest

# Rebuild backend
cd /path/to/nasiko
docker build -f app/Dockerfile -t <your-dockerhub-user>/nasiko-backend:latest .
docker push <your-dockerhub-user>/nasiko-backend:latest

# Restart the deployments
kubectl rollout restart deployment k8s-service-registry -n nasiko
kubectl rollout restart deployment k8s-build-worker -n nasiko

# Wait for rollout
kubectl rollout status deployment k8s-service-registry -n nasiko --timeout=120s
kubectl rollout status deployment k8s-build-worker -n nasiko --timeout=120s
```

#### Step 2: Clean Up Old Agent Deployment
```bash
# Delete the Kubernetes resources
kubectl delete deployment <agent-deployment-name> -n nasiko-agents
kubectl delete service <agent-service-name> -n nasiko-agents

# Clean up MongoDB records
kubectl exec -n nasiko <mongodb-pod-name> -- mongosh -u root -p secretpassword \
  --authenticationDatabase admin nasiko --eval "
  db.registry.deleteOne({id: '<agent-name>'});
  db['upload-status'].deleteMany({agent_name: '<agent-name>'});
  db.agent_builds.deleteMany({agent_id: '<agent-name>'});
  db.agent_deployments.deleteMany({agent_id: '<agent-name>'});
  db.agent_permissions.deleteMany({agent_id: '<agent-name>'});
"
```

#### Step 3: Re-upload Agent
After cleaning up, re-upload the agent through the web UI. The new deployment will have the correct `agent-name` label, and the service registry will automatically create the proper Kong route.

#### Step 4: Verify Fix
```bash
# Check service has correct label
kubectl get service -n nasiko-agents -l agent-name=<agent-name> -o yaml

# Check service registry logs
kubectl logs -n nasiko -l app=k8s-service-registry --tail=20

# Should show: "route: /agents/<agent-name>" (not /agents/agent-<name>-<timestamp>)

# Test the route from within cluster
kubectl run -it --rm debug --image=curlimages/curl --restart=Never -- \
  curl -v http://<gateway-ip>/agents/<agent-name>/health
```

**Prevention:**
- Ensure you're using the latest version of the codebase with the Kong routing fix
- When deploying agents, verify the service has the `agent-name` label
- Monitor service registry logs during agent deployment to confirm correct route creation

---

### Problem: Services Can't Reach BuildKit

**Symptoms:**
```
Error: failed to connect to buildkitd: connection refused
```

**Diagnosis:**
```bash
# Check buildkit service
kubectl get svc -n buildkit

# Test connectivity from a pod
kubectl run -it --rm debug --image=nicolaka/netshoot --restart=Never -- \
  nc -zv buildkitd.buildkit.svc.cluster.local 1234
```

**Solutions:**

#### 1. Service Not Exposed
```bash
# Ensure buildkit service exists
kubectl get svc buildkitd -n buildkit

# If missing, check buildkit deployment
kubectl describe deployment buildkitd -n buildkit
```

#### 2. Wrong Service Address
Check service address in backend configuration:
```bash
kubectl get deployment -n nasiko -o yaml | grep BUILDKIT_ADDRESS
# Should be: tcp://buildkitd.buildkit.svc.cluster.local:1234
```

---

## Resource Issues

### Problem: Cluster Out of Resources

**Symptoms:**
```
0/2 nodes are available: 2 Insufficient cpu, 2 Insufficient memory
```

**Diagnosis:**
```bash
# Check node resources
kubectl top nodes
kubectl describe nodes | grep -A 5 "Allocated resources"

# Check pod resource requests
kubectl describe pod <pod-name> -n <namespace> | grep -A 5 "Requests"
```

**Solutions:**

#### 1. Scale Up Cluster
```bash
# DigitalOcean
doctl kubernetes cluster node-pool update <cluster-id> <pool-id> --count 3

# AWS EKS
aws eks update-nodegroup-config --cluster-name <cluster> --nodegroup-name <group> --scaling-config minSize=3,maxSize=5,desiredSize=3
```

#### 2. Reduce Resource Requests
Edit deployment to lower resource requests if they're too aggressive.

#### 3. Clean Up Unused Resources
```bash
# Delete failed jobs
kubectl delete jobs -n nasiko-agents --field-selector status.successful=0

# Delete completed jobs older than 1 hour
kubectl delete jobs -n nasiko-agents --field-selector status.successful=1

# Delete evicted pods
kubectl get pods -A | grep Evicted | awk '{print $1, $2}' | xargs -n2 kubectl delete pod -n
```

---

## General Debugging Commands

### Cluster Health
```bash
# Check cluster info
kubectl cluster-info

# Check nodes
kubectl get nodes
kubectl describe nodes

# Check all resources in namespace
kubectl get all -n nasiko
kubectl get all -n nasiko-agents
kubectl get all -n buildkit
```

### Pod Debugging
```bash
# Get pod logs
kubectl logs <pod-name> -n <namespace>
kubectl logs <pod-name> -n <namespace> -c <container-name>  # For multi-container pods
kubectl logs <pod-name> -n <namespace> --previous  # Previous container logs

# Follow logs in real-time
kubectl logs -f <pod-name> -n <namespace>

# Describe pod for events
kubectl describe pod <pod-name> -n <namespace>

# Execute command in pod
kubectl exec -it <pod-name> -n <namespace> -- /bin/bash

# Check pod YAML
kubectl get pod <pod-name> -n <namespace> -o yaml
```

### Job Debugging
```bash
# List jobs
kubectl get jobs -n nasiko-agents

# Get job status
kubectl describe job <job-name> -n nasiko-agents

# Get job logs (from pod)
kubectl logs -l job-name=<job-name> -n nasiko-agents

# Delete job and its pods
kubectl delete job <job-name> -n nasiko-agents
```

### Service & Network Debugging
```bash
# List services
kubectl get svc -n <namespace>

# Check service endpoints
kubectl get endpoints -n <namespace>

# Test DNS resolution
kubectl run -it --rm debug --image=nicolaka/netshoot --restart=Never -- \
  nslookup buildkitd.buildkit.svc.cluster.local

# Test connectivity
kubectl run -it --rm debug --image=nicolaka/netshoot --restart=Never -- \
  curl -v http://buildkitd.buildkit.svc.cluster.local:1234
```

### Secret Debugging
```bash
# List secrets
kubectl get secrets -n <namespace>

# Describe secret (without revealing data)
kubectl describe secret <secret-name> -n <namespace>

# View secret data (base64 encoded)
kubectl get secret <secret-name> -n <namespace> -o yaml

# Decode specific secret field
kubectl get secret <secret-name> -n <namespace> -o jsonpath='{.data.<key>}' | base64 -d
```

### Events and Logs
```bash
# Watch events in namespace
kubectl get events -n <namespace> --watch

# Get recent events sorted by time
kubectl get events -n <namespace> --sort-by='.lastTimestamp'

# Get events for specific resource
kubectl describe <resource-type> <resource-name> -n <namespace>
```

---

## Common Resolution Patterns

### Pattern 1: Reset Registry Credentials
```bash
# Full credential reset
export KUBECONFIG=/path/to/kubeconfig.yaml

# Generate fresh credentials
DOCKER_CONFIG=$(doctl registry docker-config --read-write --expiry-seconds 31536000)

# Update nasiko-agents namespace
echo "$DOCKER_CONFIG" | kubectl delete secret agent-registry-credentials -n nasiko-agents --ignore-not-found
echo "$DOCKER_CONFIG" | kubectl create secret generic agent-registry-credentials \
  --from-file=.dockerconfigjson=/dev/stdin \
  --type=kubernetes.io/dockerconfigjson \
  -n nasiko-agents

# Update nasiko namespace
echo "$DOCKER_CONFIG" | kubectl delete secret regcred -n nasiko --ignore-not-found
echo "$DOCKER_CONFIG" | kubectl create secret generic regcred \
  --from-file=.dockerconfigjson=/dev/stdin \
  --type=kubernetes.io/dockerconfigjson \
  -n nasiko
```

### Pattern 2: Restart Deployment
```bash
# Restart deployment (rolling restart)
kubectl rollout restart deployment <deployment-name> -n <namespace>

# Check rollout status
kubectl rollout status deployment <deployment-name> -n <namespace>

# Rollback if needed
kubectl rollout undo deployment <deployment-name> -n <namespace>
```

### Pattern 3: Clean Slate Reset
```bash
# Delete failed resources
kubectl delete jobs -n nasiko-agents --all
kubectl delete pods -n nasiko-agents --field-selector=status.phase==Failed

# Restart core services
kubectl rollout restart deployment nasiko-backend -n nasiko
kubectl rollout restart deployment k8s-build-worker -n nasiko

# Wait for readiness
kubectl wait --for=condition=available --timeout=300s deployment/nasiko-backend -n nasiko
```

---

## Provider-Specific Notes

### DigitalOcean

**Registry Limitations:**
- **ONE registry per account** - You cannot have multiple registries
- Registry names must be globally unique across all DigitalOcean users
- If you try to create a registry with a name that's already taken, it will fail

**Multi-User Deployment Scenarios:**

**Scenario 1: Each team member has their own DO account**
```bash
# Each user creates their cluster and registry
# User 1:
doctl registry get  # Shows: user1-registry
uv run cli/main.py setup bootstrap --registry-type cloud --cloud-reg-name user1-registry

# User 2:
doctl registry get  # Shows: user2-registry
uv run cli/main.py setup bootstrap --registry-type cloud --cloud-reg-name user2-registry
```

**Scenario 2: Shared DO account for team**
```bash
# First user creates registry
doctl registry create team-nasiko-registry

# All team members use same registry name
uv run cli/main.py setup bootstrap \
  --registry-type cloud \
  --cloud-reg-name team-nasiko-registry

# All users must be authenticated with same DO account
doctl auth init  # Use shared DO token
```

**Scenario 3: Registry name conflict**
```bash
# If bootstrap fails with "Registry name mismatch"
# Check your actual registry:
doctl registry get

# Use your actual registry name:
uv run cli/main.py setup bootstrap \
  --registry-type cloud \
  --cloud-reg-name <your-actual-registry-name>
```

**Registry Token Lifecycle:**
- Tokens can be set to expire (default: 30 days if not specified)
- Use `--expiry-seconds 31536000` for 1-year validity
- Always use `--read-write` flag for BuildKit push operations

**Getting Account Info:**
```bash
doctl auth list
doctl account get
doctl registry get
```

**Registry Repository Management:**
```bash
# List repositories
doctl registry repository list

# List tags for repository
doctl registry repository list-tags <registry-name>/<repository-name>

# Delete repository
doctl registry repository delete <registry-name>/<repository-name> --force
```

### AWS ECR

**Token Expiry:**
- ECR tokens expire after 12 hours
- Use ECR credential refresher CronJob (deployed by setup script)
- Alternative: Use ECR credential helper in BuildKit

**Debugging ECR:**
```bash
# Get login password
aws ecr get-login-password --region <region>

# Describe repository
aws ecr describe-repositories --region <region>

# List images
aws ecr list-images --repository-name <repo-name> --region <region>
```

---

## Need More Help?

1. **Check Logs:** Always start with pod/job logs for detailed error messages
2. **Events:** Use `kubectl describe` to see recent events for a resource
3. **Documentation:** Refer to `/docs` directory for architecture and setup details
4. **GitHub Issues:** Report persistent issues at https://github.com/arithmic/nasiko/issues
