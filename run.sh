#!/usr/bin/env bash
set -euo pipefail

# Single-node dev chain with dummy Cardano config.
# Blocks produced in fallback mode initially; ticket mode after first epoch.
#
# Usage: ./run.sh [--size-tiny|--size-small|--size-full] [extra node args...]
#   default: size-tiny (6 validators max, epoch=6 slots)

SIZE_FEATURE=""
EXTRA_ARGS=()
for arg in "$@"; do
	case "$arg" in
		--size-tiny)  SIZE_FEATURE="--features size-tiny" ;;
		--size-small) SIZE_FEATURE="--features size-small" ;;
		--size-full)  SIZE_FEATURE="--features size-full" ;;
		*)            EXTRA_ARGS+=("$arg") ;;
	esac
done

cargo build -p partner-chains-demo-node --release "$SIZE_FEATURE"

export GENESIS_UTXO="0000000000000000000000000000000000000000000000000000000000000000#0"
export SLOTS_PER_EPOCH=10

# Mainchain epoch config
export MC__FIRST_EPOCH_TIMESTAMP_MILLIS=0
export MC__EPOCH_DURATION_MILLIS=600000
export MC__FIRST_EPOCH_NUMBER=0
export MC__FIRST_SLOT_NUMBER=0
export MC__SLOT_DURATION_MILLIS=6000

# Committee selection scripts
export COMMITTEE_CANDIDATE_ADDRESS="addr_test1wz00000000000000000000000000000000000000000000ckcsaul"
export D_PARAMETER_POLICY_ID="00000000000000000000000000000000000000000000000000000000"
export PERMISSIONED_CANDIDATES_POLICY_ID="00000000000000000000000000000000000000000000000000000000"

# Governed map scripts
export GOVERNED_MAP_POLICY_ID="00000000000000000000000000000000000000000000000000000000"
export GOVERNED_MAP_VALIDATOR_ADDRESS="addr_test1wz00000000000000000000000000000000000000000000ckcsaul"

# Bridge scripts
export BRIDGE_TOKEN_POLICY_ID="00000000000000000000000000000000000000000000000000000000"
export BRIDGE_TOKEN_ASSET_NAME="00"
export ILLIQUID_CIRCULATION_SUPPLY_VALIDATOR_ADDRESS="addr_test1wz00000000000000000000000000000000000000000000ckcsaul"

# Data source (mock = no real Cardano needed)
export CARDANO_DATA_SOURCE=mock
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
export MOCK_REGISTRATIONS_FILE="${SCRIPT_DIR}/dev-mock-registrations.json"

exec ./target/release/partner-chains-demo-node --dev --tmp --validator "${EXTRA_ARGS[@]}"
