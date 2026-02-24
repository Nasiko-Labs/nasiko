#!/bin/bash

set -e

echo "🚀 Starting Kong API Gateway for Nasiko Agents..."

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

log_info() {
    echo -e "${BLUE}[INFO]${NC} $1"
}

log_success() {
    echo -e "${GREEN}[SUCCESS]${NC} $1"
}

log_warning() {
    echo -e "${YELLOW}[WARNING]${NC} $1"
}

log_error() {
    echo -e "${RED}[ERROR]${NC} $1"
}

# Check if docker is running
if ! docker info >/dev/null 2>&1; then
    log_error "Docker is not running. Please start Docker first."
    exit 1
fi

# Create agents network if it doesn't exist
log_info "Checking for agents-net network..."
if ! docker network inspect agents-net >/dev/null 2>&1; then
    log_warning "agents-net network not found. Creating it..."
    docker network create agents-net
    log_success "agents-net network created"
else
    log_success "agents-net network exists"
fi

# Start Kong and related services
log_info "Starting Kong API Gateway..."
cd "$(dirname "$0")"
docker-compose up -d

# Wait for Kong to be ready
log_info "Waiting for Kong to be ready..."
max_attempts=30
attempt=1

while [ $attempt -le $max_attempts ]; do
    if curl -s http://localhost:9101/status >/dev/null 2>&1; then
        log_success "Kong is ready!"
        break
    fi

    echo -n "."
    sleep 2
    attempt=$((attempt + 1))
done

if [ $attempt -gt $max_attempts ]; then
    log_error "Kong failed to start after $((max_attempts * 2)) seconds"
    log_error "Check logs with: docker-compose logs kong"
    exit 1
fi

# Wait for service registry to be ready
log_info "Waiting for Service Registry to be ready..."
max_attempts=15
attempt=1

while [ $attempt -le $max_attempts ]; do
    if curl -s http://localhost:8080/health >/dev/null 2>&1; then
        log_success "Service Registry is ready!"
        break
    fi

    echo -n "."
    sleep 2
    attempt=$((attempt + 1))
done

if [ $attempt -gt $max_attempts ]; then
    log_warning "Service Registry may not be fully ready yet"
fi

# Wait for chat history service to be ready
log_info "Waiting for Chat History Service to be ready..."
max_attempts=15
attempt=1

while [ $attempt -le $max_attempts ]; do
    if curl -s http://localhost:8083/health >/dev/null 2>&1; then
        log_success "Chat History Service is ready!"
        break
    fi

    echo -n "."
    sleep 2
    attempt=$((attempt + 1))
done

if [ $attempt -gt $max_attempts ]; then
    log_warning "Chat History Service may not be fully ready yet"
fi

# Configure Kong plugins
log_info "Configuring Kong plugins..."

# Check if chat-logger plugin already exists
EXISTING_PLUGIN=$(curl -s http://localhost:9101/plugins | jq -r '.data[] | select(.name == "chat-logger") | .id')

if [ -z "$EXISTING_PLUGIN" ] || [ "$EXISTING_PLUGIN" = "null" ]; then
    log_info "Installing chat-logger plugin..."
    
    PLUGIN_RESPONSE=$(curl -s -w "%{http_code}" -o /tmp/kong_plugin_response.json \
        -X POST http://localhost:9101/plugins \
        -H "Content-Type: application/json" \
        -d '{
            "name": "chat-logger",
            "config": {
                "chat_service_url": "http://chat-history-service:8002",
                "timeout": 5000
            }
        }')
    
    HTTP_CODE="${PLUGIN_RESPONSE: -3}"
    
    if [[ "$HTTP_CODE" =~ ^(200|201)$ ]]; then
        PLUGIN_ID=$(cat /tmp/kong_plugin_response.json | jq -r '.id')
        log_success "chat-logger plugin installed successfully! (ID: $PLUGIN_ID)"
    else
        log_error "Failed to install chat-logger plugin (HTTP: $HTTP_CODE)"
        log_error "Response: $(cat /tmp/kong_plugin_response.json)"
    fi
    
    rm -f /tmp/kong_plugin_response.json
else
    log_success "chat-logger plugin already installed (ID: $EXISTING_PLUGIN)"
fi

echo
log_success "Kong API Gateway is running!"
echo
echo "📋 Access Points:"
echo "  Kong Proxy (API Gateway):     http://localhost:9100"
echo "  Kong Admin API:               http://localhost:9101"
echo "  Kong Manager (GUI):           http://localhost:9102"
echo "  Konga Dashboard:              http://localhost:1337"
echo "  Service Registry Status:      http://localhost:8080/status"
echo "  Chat History Service:         http://localhost:8083/health"
echo
echo "🔍 Usage Examples:"
echo "  # Instead of direct agent access:"
echo "  curl http://localhost:5000/translate"
echo
echo "  # Use Kong routes:"
echo "  curl http://localhost:9100/translator/translate"
echo
echo "📊 Monitoring:"
echo "  # Check discovered services:"
echo "  curl http://localhost:8080/services"
echo
echo "  # View Kong services:"
echo "  curl http://localhost:9101/services"
echo
echo "  # View Kong routes:"
echo "  curl http://localhost:9101/routes"
echo
echo "🔌 Active Plugins:"
echo "  chat-logger: Automatically logs JSONRPC agent conversations"
echo
echo "🎯 Next Steps:"
echo "  1. Start your agent containers"
echo "  2. Wait 30 seconds for auto-discovery"
echo "  3. Check http://localhost:8080/services to see discovered agents"
echo "  4. Use Kong routes: http://localhost:9100/{agent-name}/{endpoint}"
echo "  5. Chat history will be automatically logged to MongoDB"
echo