#!/usr/bin/env bash
# MAIC model health check for Miracle Claw
# Verifies the 7 MC-relevant models are still exposed by MAIC API.
# Run daily from cron (recommended: 09:00 MDT).
#
# Source MAIC_API_KEY + MAIC_BASE from ~/.config/secrets/maic.env
# (created by HomeBot's refresh_maic_jwt.sh).

set -euo pipefail

if [[ -f "$HOME/.config/secrets/maic.env" ]]; then
    # shellcheck disable=SC1091
    . "$HOME/.config/secrets/maic.env"
fi

MAIC_BASE="${MAIC_BASE:-https://api.maicserver.com}"
MC_RELEVANT=(
    milagro-dev         # the 14B — MC default
    milagro-dev-coder   # 14B coder
    milagro-oc-qwen     # cloud cascade: qwen
    milagro-oc-deepseek # cloud cascade: deepseek
    milagro-oc-glm      # cloud cascade: glm
    milagro-oc-kimi     # cloud cascade: kimi
    milagro-oc-minimax  # cloud cascade: minimax
)

if [[ -z "${MAIC_API_KEY:-}" ]]; then
    echo "ERROR: MAIC_API_KEY not set. Source ~/.config/secrets/maic.env first." >&2
    exit 2
fi

response=$(curl -s --max-time 10 -H "Authorization: Bearer $MAIC_API_KEY" "$MAIC_BASE/v1/models")
if [[ $? -ne 0 ]]; then
    echo "ERROR: curl to $MAIC_BASE/v1/models failed" >&2
    exit 3
fi

total=$(echo "$response" | python3 -c "import json, sys; print(len(json.load(sys.stdin)['data']))")

missing=0
for m in "${MC_RELEVANT[@]}"; do
    if echo "$response" | grep -q "\"id\":\"$m\""; then
        echo "[OK] $m"
    else
        echo "[MISSING] $m"
        missing=$((missing + 1))
    fi
done

echo "---"
echo "Total models on MAIC: $total"
echo "MC-relevant: ${#MC_RELEVANT[@]}"
echo "Missing: $missing"

if [[ $missing -gt 0 ]]; then
    echo "ACTION: escalate via OpenClaw channels — drop note in MC STATUS.md"
    exit 1
fi
