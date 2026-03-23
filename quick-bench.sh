#!/bin/bash
# Quick benchmark - just the essentials
# Usage: ./quick-bench.sh

set -e

echo "OSAgent Quick Benchmark"
echo "======================="

# Build if needed
if [ ! -f ./target/release/osagent ]; then
    echo "Building..."
    cargo build --release 2>/dev/null
fi

# Binary size
SIZE=$(ls -lh ./target/release/osagent | awk '{print $5}')
echo "Binary:    $SIZE"

# Startup (5 samples)
SUM=0
for i in {1..5}; do
    MS=$( { time ./target/release/osagent --version > /dev/null 2>&1; } 2>&1 | grep real | sed 's/real//' | tr -d '\t' | awk -F'[ms]' '{print ($1*1000)+$2}')
    SUM=$((SUM + MS))
done
echo "Startup:   $((SUM/5))ms avg"

# Memory
./target/release/osagent --version &
PID=$!
sleep 0.5
if [[ "$OSTYPE" == "darwin"* ]]; then
    MEM=$(ps -o rss= -p $PID | awk '{printf "%.0f", $1/1024}')
else
    MEM=$(ps -o rss= -p $PID | awk '{printf "%.0f", $1/1024}')
fi
kill $PID 2>/dev/null || true
echo "Memory:    ${MEM}MB"

echo ""
echo "Targets:   <50ms startup, <30MB memory, <15MB binary"
