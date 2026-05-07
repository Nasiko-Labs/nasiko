#!/bin/bash
# One-Command Gateway Setup Script
# Usage: ./setup-gateway.sh

set -e

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

echo -e "${BLUE}========================================${NC}"
echo -e "${BLUE}  LLM Gateway Setup Script${NC}"
echo -e "${BLUE}========================================${NC}"
echo ""

# Get script directory
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"

# Check prerequisites
echo -e "${YELLOW}Checking prerequisites...${NC}"

# Check Docker
if ! command -v docker &> /dev/null; then
    echo -e "${RED}Error: Docker not found${NC}"
    echo "Please install Docker: https://docs.docker.com/get-docker/"
    exit 1
fi

# Check Docker Compose
if ! command -v docker-compose &> /dev/null; then
    echo -e "${RED}Error: Docker Compose not found${NC}"
    echo "Please install Docker Compose: https://docs.docker.com/compose/install/"
    exit 1
fi

echo -e "${GREEN}✓ Docker and Docker Compose found${NC}"

# Check if .env.gateway exists
if [ ! -f "$PROJECT_DIR/.env.gateway" ]; then
    echo -e "${YELLOW}.env.gateway not found. Creating from template...${NC}"
    
    if [ -f "$PROJECT_DIR/.env.gateway.example" ]; then
        cp "$PROJECT_DIR/.env.gateway.example" "$PROJECT_DIR/.env.gateway"
        echo -e "${GREEN}✓ Created .env.gateway from template${NC}"
        echo ""
        echo -e "${YELLOW}⚠️  IMPORTANT: Please edit .env.gateway with your actual API keys!${NC}"
        echo ""
        echo "Required variables to set:"
        echo "  - OPENAI_API_KEY"
        echo "  - ANTHROPIC_API_KEY"
        echo "  - LITELLM_MASTER_KEY"
        echo "  - POSTGRES_PASSWORD"
        echo ""
        echo "After editing, run this script again."
        exit 0
    else
        echo -e "${RED}Error: .env.gateway.example not found${NC}"
        exit 1
    fi
else
    echo -e "${GREEN}✓ .env.gateway found${NC}"
fi

# Check required environment variables
echo ""
echo -e "${YELLOW}Checking environment variables...${NC}"

# Source the env file
set -a
source "$PROJECT_DIR/.env.gateway"
set +a

required_vars=("OPENAI_API_KEY" "LITELLM_MASTER_KEY")
missing_vars=()

for var in "${required_vars[@]}"; do
    if [ -z "${!var}" ]; then
        missing_vars+=("$var")
    fi
done

if [ ${#missing_vars[@]} -ne 0 ]; then
    echo -e "${RED}Error: Missing required environment variables:${NC}"
    for var in "${missing_vars[@]}"; do
        echo "  - $var"
    done
    echo ""
    echo "Please set these in .env.gateway and run again."
    exit 1
fi

echo -e "${GREEN}✓ All required variables set${NC}"

# Create necessary directories
echo ""
echo -e "${YELLOW}Creating directories...${NC}"
mkdir -p "$PROJECT_DIR/logs"
echo -e "${GREEN}✓ Directories created${NC}"

# Start services
echo ""
echo -e "${BLUE}========================================${NC}"
echo -e "${BLUE}  Starting LLM Gateway${NC}"
echo -e "${BLUE}========================================${NC}"
echo ""

cd "$PROJECT_DIR"
docker-compose -f docker/docker-compose.gateway.yml up -d

echo ""
echo -e "${YELLOW}Waiting for services to be healthy...${NC}"

# Wait for gateway to be ready
attempt=0
max_attempts=30
while [ $attempt -lt $max_attempts ]; do
    if curl -s http://localhost:4000/health > /dev/null 2>&1; then
        echo -e "${GREEN}✓ Gateway is ready!${NC}"
        break
    fi
    
    attempt=$((attempt + 1))
    echo "  Attempt $attempt/$max_attempts..."
    sleep 2
done

if [ $attempt -eq $max_attempts ]; then
    echo -e "${RED}Error: Gateway failed to start${NC}"
    echo "Check logs with: docker-compose -f docker/docker-compose.gateway.yml logs"
    exit 1
fi

# Show status
echo ""
echo -e "${BLUE}========================================${NC}"
echo -e "${BLUE}  Gateway Status${NC}"
echo -e "${BLUE}========================================${NC}"
echo ""

docker-compose -f docker/docker-compose.gateway.yml ps

echo ""
echo -e "${GREEN}========================================${NC}"
echo -e "${GREEN}  Setup Complete!${NC}"
echo -e "${GREEN}========================================${NC}"
echo ""
echo "Gateway URL: http://localhost:4000"
echo "Health Check: http://localhost:4000/health"
echo ""
echo -e "${YELLOW}Next Steps:${NC}"
echo ""
echo "1. Generate a virtual key for your team:"
echo "   make generate-key team=myteam"
echo ""
echo "2. Set the virtual key in your environment:"
echo "   export GATEWAY_VIRTUAL_KEY='your-key-here'"
echo ""
echo "3. Test the gateway agent:"
echo "   make test-agent-gateway"
echo ""
echo "4. View gateway logs:"
echo "   make gateway-logs"
echo ""
echo -e "${BLUE}========================================${NC}"
