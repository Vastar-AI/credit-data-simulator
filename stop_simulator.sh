#!/bin/bash

# Stop Credit Data Simulator — kills all processes on ports 18081-18084

RED='\033[0;31m'
GREEN='\033[0;32m'
NC='\033[0m'

echo "Stopping Credit Data Simulator..."

pkill -f "credit-data-simulator" 2>/dev/null || true
pkill -f "simulators-poc" 2>/dev/null || true

for PORT in 18081 18082 18083 18084; do
    PID=$(lsof -ti:${PORT} 2>/dev/null || true)
    if [ -n "$PID" ]; then
        kill -9 $PID 2>/dev/null || true
        echo -e "${GREEN}  Killed PID ${PID} on port ${PORT}${NC}"
    fi
done

echo -e "${GREEN}Done.${NC}"
