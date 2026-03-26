#!/bin/bash

# =============================================================================
# Credit Data Simulator — Runner
# =============================================================================
# Starts 4 services: Core Banking, Mapping, Rulepack, Regulator
# Auto-kills any existing processes on required ports before starting.
# =============================================================================

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BINARY="${SCRIPT_DIR}/target/release/credit-data-simulator"
PORTS=(18081 18082 18083 18084)

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
CYAN='\033[0;36m'
NC='\033[0m'

echo -e "${GREEN}=== Credit Data Simulator ===${NC}"
echo ""

# --- Port cleanup ---
echo -e "${YELLOW}Checking ports...${NC}"
for PORT in "${PORTS[@]}"; do
    PID=$(lsof -ti:${PORT} 2>/dev/null || true)
    if [ -n "$PID" ]; then
        echo -e "${YELLOW}  Killing PID ${PID} on port ${PORT}${NC}"
        kill -9 $PID 2>/dev/null || true
    fi
done

# Also kill by process name
pkill -9 -f "credit-data-simulator" 2>/dev/null || true
pkill -9 -f "simulators-poc" 2>/dev/null || true
sleep 1

# Verify all ports free
ALL_FREE=true
for PORT in "${PORTS[@]}"; do
    if lsof -ti:${PORT} >/dev/null 2>&1; then
        echo -e "${RED}  Port ${PORT} still in use${NC}"
        ALL_FREE=false
    fi
done

if [ "$ALL_FREE" = true ]; then
    echo -e "${GREEN}  All ports free (${PORTS[*]})${NC}"
else
    echo -e "${RED}  Failed to free all ports. Aborting.${NC}"
    exit 1
fi

# --- Build if needed ---
if [ ! -f "$BINARY" ]; then
    echo ""
    echo -e "${YELLOW}Binary not found. Building...${NC}"
    cd "$SCRIPT_DIR"
    cargo build --release
fi

# --- Print info ---
echo ""
echo -e "${CYAN}Services:${NC}"
echo "  Core Banking       http://localhost:18081   (NDJSON credit data, pagination, dirty ratio)"
echo "  Mapping Service    http://localhost:18082   (SLIK field mapping versions)"
echo "  Rulepack Service   http://localhost:18083   (Validation rules engine)"
echo "  Regulator Endpoint http://localhost:18084   (OJK submission simulator)"
echo ""
echo -e "${CYAN}Quick test (curl):${NC}"
echo "  curl http://localhost:18081/health"
echo "  curl http://localhost:18081/api/v1/credits/count"
echo "  curl 'http://localhost:18081/api/v1/credits/ndjson?page_size=10' | head -3"
echo "  curl http://localhost:18082/api/v1/mappings"
echo "  curl http://localhost:18083/api/v1/rulepacks"
echo "  curl http://localhost:18084/health"
echo ""
echo -e "${CYAN}Benchmark (oha):${NC}"
echo "  # NDJSON streaming (100 records/page)"
echo "  oha -c 200 -n 2000 'http://localhost:18081/api/v1/credits/ndjson?page_size=100'"
echo ""
echo "  # Paginated JSON"
echo "  oha -c 200 -n 2000 'http://localhost:18081/api/v1/credits?page_size=100'"
echo ""

# --- Start ---
echo -e "${GREEN}Starting simulator...${NC}"
echo ""
exec "$BINARY"
