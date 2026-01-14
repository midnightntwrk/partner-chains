#!/usr/bin/env python3
import subprocess
import json
import sys
import time
import random
import concurrent.futures
import math
import shutil
import tempfile
import os
import argparse

# Configuration
TOOLKIT_CMD = "midnight-node-toolkit"
SRC_URL = "ws://ferdie.node.sc.iog.io:9944"
DEST_URL = "ws://ferdie.node.sc.iog.io:9944"
SOURCE_SEEDS = [
    "0000000000000000000000000000000000000000000000000000000000000001",
    "0000000000000000000000000000000000000000000000000000000000000002",
    "0000000000000000000000000000000000000000000000000000000000000003"
]
TOKEN_TYPE = "0000000000000000000000000000000000000000000000000000000000000000"
AMOUNT = 1000000*10**6
START_INDEX = 10
END_INDEX = 99
DB_PATH = "toolkit.db"

def run_command(cmd, cwd=None):
    """Runs a command and returns stdout if successful, exits otherwise."""
    try:
        result = subprocess.run(cmd, capture_output=True, text=True, check=True, cwd=cwd)
        return result.stdout.strip()
    except subprocess.CalledProcessError as e:
        print(f"\n❌ Error executing command: {' '.join(cmd)}")
        print(f"STDOUT: {e.stdout}")
        print(f"STDERR: {e.stderr}")
        raise e
    except FileNotFoundError:
        print(f"\n❌ Error: Executable '{cmd[0]}' not found. Ensure it is in your PATH.")
        sys.exit(1)

def get_wallet_address(index, cwd=None):
    """Creates a wallet seed and retrieves its address."""
    # Seed format: 00..xx padded to 64 chars
    seed = f"{index:064}"

    cmd = [
        TOOLKIT_CMD, "show-address",
        "--network", "undeployed",
        "--seed", seed
    ]

    output = run_command(cmd, cwd=cwd)
    try:
        data = json.loads(output)
        return data["unshielded"]
    except json.JSONDecodeError:
        print(f"\n❌ Failed to parse JSON from show-address output: {output}")
        sys.exit(1)
    except KeyError:
        print(f"\n❌ JSON output does not contain 'unshielded' field: {output}")
        sys.exit(1)

def fund_address(address, funding_seed, cwd=None):
    """Funds the given address using the source seed."""

    cmd = [
        TOOLKIT_CMD, "generate-txs", "single-tx",
        "--source-seed", funding_seed,
        "--src-url", SRC_URL,
        "--unshielded-amount", str(AMOUNT),
        "--unshielded-token-type", TOKEN_TYPE,
        "--destination-address", address,
        "--dest-url", DEST_URL
    ]

    # Run the command (output is captured but we assume success if no error raised)
    run_command(cmd, cwd=cwd)

def process_chunk(chunk_start, chunk_end, funding_seed):
    print(f"🚀 Starting chunk {chunk_start}-{chunk_end} with funding seed ...{funding_seed[-2:]}")

    with tempfile.TemporaryDirectory() as temp_dir:
        # Copy toolkit.db to temp_dir to avoid locking
        if os.path.exists(DB_PATH):
            shutil.copy(DB_PATH, os.path.join(temp_dir, "toolkit.db"))

        for i in range(chunk_start, chunk_end + 1):
            try:
                print(f"[Chunk {funding_seed[-2:]}] Generating wallet {i}...", end=" ", flush=True)
                addr = get_wallet_address(i, cwd=temp_dir)
                print(f"✅ {addr}")

                print(f"[Chunk {funding_seed[-2:]}] Funding {addr}...", end=" ", flush=True)
                fund_address(addr, funding_seed, cwd=temp_dir)
                print("✅ Sent")

                # Wait a bit between transactions to ensure nonce propagation
                time.sleep(2)
            except Exception as e:
                print(f"\n❌ Failed processing index {i}: {e}")

def main():
    parser = argparse.ArgumentParser(description="Fund wallets.")
    parser.add_argument("--start", type=int, default=START_INDEX, help="Starting seed to be funded")
    parser.add_argument("--end", type=int, default=END_INDEX, help="Ending seed to be funded")
    args = parser.parse_args()

    start_index = args.start
    end_index = args.end

    print("🚀 Starting wallet creation and funding script...")

    total_wallets = end_index - start_index + 1
    num_workers = len(SOURCE_SEEDS)
    chunk_size = math.ceil(total_wallets / num_workers)

    start_time = time.time()

    with concurrent.futures.ThreadPoolExecutor(max_workers=num_workers) as executor:
        futures = []
        for i in range(num_workers):
            chunk_start = start_index + i * chunk_size
            chunk_end = min(start_index + (i + 1) * chunk_size - 1, end_index)

            if chunk_start > chunk_end:
                break

            futures.append(executor.submit(process_chunk, chunk_start, chunk_end, SOURCE_SEEDS[i]))

        concurrent.futures.wait(futures)

    end_time = time.time()
    total_duration = end_time - start_time

    print("\n🎉 All operations completed successfully.")
    if total_duration > 120:
        minutes = int(total_duration // 60)
        seconds = total_duration % 60
        print(f"⏱️ Total execution time for {total_wallets} wallets: {minutes} minutes and {seconds:.2f} seconds")
    else:
        print(f"⏱️ Total execution time for {total_wallets} wallets: {total_duration:.2f} seconds")
    if total_wallets > 0:
        print(f"📊 Average time per funding: {total_duration / total_wallets:.2f} seconds")

if __name__ == "__main__":
    main()
