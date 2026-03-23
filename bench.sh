#!/bin/bash
# OSAgent Performance Benchmark Suite
# Run with: ./bench.sh

set -e

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
NC='\033[0m' # No Color
BOLD='\033[1m'

RESULTS_DIR="./benchmark_results"
TIMESTAMP=$(date +%Y%m%d_%H%M%S)
REPORT_FILE="$RESULTS_DIR/report_$TIMESTAMP.md"

mkdir -p "$RESULTS_DIR"

echo -e "${CYAN}${BOLD}"
echo "╔═══════════════════════════════════════════════════════════════╗"
echo "║         OSAgent Performance Benchmark Suite                   ║"
echo "╚═══════════════════════════════════════════════════════════════╝"
echo -e "${NC}"

# CI-quality checks
echo -e "${YELLOW}[1/7] Running CI-quality checks...${NC}"
cargo fmt -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features --verbose

# Build release binary
echo -e "${YELLOW}[2/7] Building release binary...${NC}"
cargo build --release 2>/dev/null
BINARY_SIZE=$(ls -lh ./target/release/osagent | awk '{print $5}')
echo -e "   Binary size: ${GREEN}$BINARY_SIZE${NC}"

# Memory baseline
echo -e "${YELLOW}[3/7] Measuring memory baseline...${NC}"
./target/release/osagent --version &
PID=$!
sleep 0.5
if command -v ps &> /dev/null; then
    if [[ "$OSTYPE" == "darwin"* ]]; then
        MEM_MB=$(ps -o rss= -p $PID | awk '{printf "%.1f", $1/1024}')
    else
        MEM_MB=$(ps -o rss= -p $PID | awk '{printf "%.1f", $1/1024}')
    fi
    echo -e "   Idle memory: ${GREEN}${MEM_MB}MB${NC}"
else
    MEM_MB="N/A"
fi
kill $PID 2>/dev/null || true

# Startup time
echo -e "${YELLOW}[4/7] Measuring startup time...${NC}"
STARTUP_TIMES=()
for i in {1..10}; do
    START=$(date +%s%N)
    ./target/release/osagent --version > /dev/null 2>&1
    END=$(date +%s%N)
    MS=$(( (END - START) / 1000000 ))
    STARTUP_TIMES+=($MS)
done
AVG_STARTUP=$(echo "${STARTUP_TIMES[@]}" | tr ' ' '\n' | awk '{sum+=$1} END {printf "%.1f", sum/NR}')
echo -e "   Average startup: ${GREEN}${AVG_STARTUP}ms${NC}"

# Run criterion benchmarks
echo -e "${YELLOW}[5/7] Running micro-benchmarks...${NC}"
cargo bench --bench performance -- --save-baseline osagent_$TIMESTAMP 2>&1 | tee "$RESULTS_DIR/criterion_$TIMESTAMP.txt" || true

# Compare with competitors if available
echo -e "${YELLOW}[6/7] Checking competitors...${NC}"

# OpenCode comparison
if command -v opencode &> /dev/null; then
    echo -e "   ${BLUE}Found OpenCode, comparing...${NC}"
    OPCODE_STARTUP_TIMES=()
    for i in {1..5}; do
        START=$(date +%s%N)
        opencode --version > /dev/null 2>&1
        END=$(date +%s%N)
        MS=$(( (END - START) / 1000000 ))
        OPCODE_STARTUP_TIMES+=($MS)
    done
    OPCODE_AVG=$(echo "${OPCODE_STARTUP_TIMES[@]}" | tr ' ' '\n' | awk '{sum+=$1} END {printf "%.1f", sum/NR}')
    
    # Get OpenCode memory
    opencode --version &
    OPCODE_PID=$!
    sleep 1
    if [[ "$OSTYPE" == "darwin"* ]]; then
        OPCODE_MEM=$(ps -o rss= -p $OPCODE_PID | awk '{printf "%.1f", $1/1024}')
    else
        OPCODE_MEM=$(ps -o rss= -p $OPCODE_PID | awk '{printf "%.1f", $1/1024}')
    fi
    kill $OPCODE_PID 2>/dev/null || true
    
    echo -e "   OpenCode startup: ${OPCODE_AVG}ms, memory: ${OPCODE_MEM}MB"
else
    echo -e "   ${RED}OpenCode not found (install with: npm i -g opencode-ai)${NC}"
    OPCODE_AVG="N/A"
    OPCODE_MEM="N/A"
fi

# Claude Code comparison
if command -v claude &> /dev/null; then
    echo -e "   ${BLUE}Found Claude Code, comparing...${NC}"
    CLAUDE_STARTUP_TIMES=()
    for i in {1..5}; do
        START=$(date +%s%N)
        claude --version > /dev/null 2>&1
        END=$(date +%s%N)
        MS=$(( (END - START) / 1000000 ))
        CLAUDE_STARTUP_TIMES+=($MS)
    done
    CLAUDE_AVG=$(echo "${CLAUDE_STARTUP_TIMES[@]}" | tr ' ' '\n' | awk '{sum+=$1} END {printf "%.1f", sum/NR}')
    echo -e "   Claude Code startup: ${CLAUDE_AVG}ms"
else
    echo -e "   ${RED}Claude Code not found${NC}"
    CLAUDE_AVG="N/A"
fi

# Generate report
echo -e "${YELLOW}[7/7] Generating report...${NC}"

cat > "$REPORT_FILE" << EOF
# OSAgent Performance Report

**Generated:** $(date)
**Platform:** $(uname -s) $(uname -m)
**Rust Version:** $(rustc --version)

---

## Summary

| Metric | OSAgent | OpenCode | Claude Code |
|--------|---------|----------|-------------|
| Startup Time | ${AVG_STARTUP}ms | ${OPCODE_AVG}ms | ${CLAUDE_AVG}ms |
| Idle Memory | ${MEM_MB}MB | ${OPCODE_MEM}MB | N/A |
| Binary Size | ${BINARY_SIZE} | N/A | N/A |

---

## Detailed Results

### Startup Time (10 runs)
$(for t in "${STARTUP_TIMES[@]}"; do echo "- ${t}ms"; done)

### Memory Profile
- **Idle**: ${MEM_MB}MB
- **Binary**: ${BINARY_SIZE}

### Micro-Benchmarks
\`\`\`
$(grep -E "^(bench_|time:)" "$RESULTS_DIR/criterion_$TIMESTAMP.txt" 2>/dev/null || echo "Run cargo bench for detailed results")
\`\`\`

---

## Performance Targets

| Metric | Target | Status |
|--------|--------|--------|
| Startup < 50ms | ${AVG_STARTUP}ms | $([ $(echo "$AVG_STARTUP < 50" | bc -l 2>/dev/null || echo "0") -eq 1 ] && echo "✅ PASS" || echo "❌ FAIL") |
| Memory < 30MB | ${MEM_MB}MB | $([ $(echo "$MEM_MB < 30" | bc -l 2>/dev/null || echo "0") -eq 1 ] && echo "✅ PASS" || echo "❌ FAIL") |
| Binary < 15MB | ${BINARY_SIZE} | $(echo "$BINARY_SIZE" | grep -qE "^[0-9]+(\.[0-9]+)?M$" && echo "⚠️ CHECK" || echo "✅ PASS") |

---

## vs OpenCode

$(if [ "$OPCODE_AVG" != "N/A" ]; then
    SPEEDUP=$(echo "scale=1; $OPCODE_AVG / $AVG_STARTUP" | bc)
    echo "**OSAgent is ${SPEEDUP}x faster at startup**"
else
    echo "OpenCode not installed"
fi)

$(if [ "$OPCODE_MEM" != "N/A" ] && [ "$MEM_MB" != "N/A" ]; then
    MEM_SAVINGS=$(echo "scale=1; ($OPCODE_MEM - $MEM_MB) / $OPCODE_MEM * 100" | bc)
    echo "**OSAgent uses ${MEM_SAVINGS}% less memory**"
fi)

---

*Run \`./bench.sh\` to regenerate this report*
EOF

echo ""
echo -e "${GREEN}${BOLD}═══════════════════════════════════════════════════════════════${NC}"
echo -e "${GREEN}${BOLD}                    BENCHMARK COMPLETE                          ${NC}"
echo -e "${GREEN}${BOLD}═══════════════════════════════════════════════════════════════${NC}"
echo ""
echo -e "Report saved to: ${CYAN}$REPORT_FILE${NC}"
echo ""
echo -e "${BOLD}Quick Summary:${NC}"
echo -e "  Startup:   ${GREEN}${AVG_STARTUP}ms${NC}"
echo -e "  Memory:    ${GREEN}${MEM_MB}MB${NC}"  
echo -e "  Binary:    ${GREEN}${BINARY_SIZE}${NC}"
echo ""

# Print comparison
if [ "$OPCODE_AVG" != "N/A" ]; then
    SPEEDUP=$(echo "scale=1; $OPCODE_AVG / $AVG_STARTUP" | bc 2>/dev/null || echo "?")
    echo -e "${BOLD}vs OpenCode:${NC}"
    echo -e "  Startup:  ${GREEN}${SPEEDUP}x faster${NC}"
fi

if [ "$OPCODE_MEM" != "N/A" ] && [ "$MEM_MB" != "N/A" ]; then
    MEM_SAVINGS=$(echo "scale=0; ($OPCODE_MEM - $MEM_MB) / $OPCODE_MEM * 100" | bc 2>/dev/null || echo "0")
    echo -e "  Memory:   ${GREEN}${MEM_SAVINGS}% less${NC}"
fi

echo ""
echo -e "View full report: ${CYAN}cat $REPORT_FILE${NC}"
