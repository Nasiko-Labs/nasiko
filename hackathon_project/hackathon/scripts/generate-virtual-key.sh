#!/bin/bash
# Generate Virtual Key for LLM Gateway
# Usage: ./generate-virtual-key.sh <team_name> [models] [budget]

set -e

# Configuration
GATEWAY_URL="${GATEWAY_URL:-http://localhost:4000}"
MASTER_KEY="${LITELLM_MASTER_KEY}"

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# Check if master key is set
if [ -z "$MASTER_KEY" ]; then
    echo -e "${RED}Error: LITELLM_MASTER_KEY not set${NC}"
    echo "Set it with: export LITELLM_MASTER_KEY='your-master-key'"
    exit 1
fi

# Parse arguments
TEAM_NAME="${1:-default}"
MODELS="${2:-gpt-4,gpt-3.5-turbo}"
BUDGET="${3:-100.0}"

# Convert comma-separated models to JSON array
MODELS_JSON=$(echo "$MODELS" | sed 's/,/","/g' | sed 's/^/["/' | sed 's/$/"]/')

echo "========================================"
echo "Generating Virtual Key"
echo "========================================"
echo "Team: $TEAM_NAME"
echo "Models: $MODELS"
echo "Budget: \$$BUDGET"
echo ""

# Generate the key
response=$(curl -s -X POST "$GATEWAY_URL/key/generate" \
    -H "Authorization: Bearer $MASTER_KEY" \
    -H "Content-Type: application/json" \
    -d "{
        \"key_name\": \"$TEAM_NAME\",
        \"models\": $MODELS_JSON,
        \"max_budget\": $BUDGET,
        \"budget_duration\": \"30d\",
        \"tpm_limit\": 100000,
        \"rpm_limit\": 1000
    }")

# Check if request was successful
if echo "$response" | grep -q "error"; then
    echo -e "${RED}Error generating key:${NC}"
    echo "$response" | jq . 2>/dev/null || echo "$response"
    exit 1
fi

# Extract key from response
key=$(echo "$response" | jq -r '.key // .data.key // empty')

if [ -z "$key" ] || [ "$key" == "null" ]; then
    echo -e "${RED}Error: Could not extract key from response${NC}"
    echo "Response: $response"
    exit 1
fi

echo -e "${GREEN}✓ Virtual key generated successfully!${NC}"
echo ""
echo "========================================"
echo "Key Details"
echo "========================================"
echo "Key: ${key:0:20}..."
echo "Full Key: $key"
echo ""
echo "Team: $TEAM_NAME"
echo "Models: $MODELS"
echo "Budget: \$$BUDGET / 30 days"
echo ""
echo "========================================"
echo "Environment Variables"
echo "========================================"
echo ""
echo "Add this to your .env file:"
echo ""
echo "# Gateway configuration for team: $TEAM_NAME"
echo "GATEWAY_URL=$GATEWAY_URL"
echo "GATEWAY_VIRTUAL_KEY=$key"
echo ""
echo "Or export directly:"
echo "  export GATEWAY_URL='$GATEWAY_URL'"
echo "  export GATEWAY_VIRTUAL_KEY='$key'"
echo ""
echo -e "${YELLOW}⚠️  IMPORTANT: Store this key securely!${NC}"
echo "   It cannot be retrieved again after generation."
echo ""
echo "========================================"
echo "Usage Example"
echo "========================================"
echo ""
echo "Python:"
echo "  from base_gateway_client import create_client"
echo "  client = create_client()"
echo "  response = client.chat.completions.create(...)"
echo ""
echo "cURL:"
echo "  curl $GATEWAY_URL/chat/completions \\"
echo "    -H \"Authorization: Bearer $key\" \\"
echo "    -H \"Content-Type: application/json\" \\"
echo "    -d '{\"model\": \"gpt-4\", \"messages\": [...]}'"
echo ""
echo "========================================"
