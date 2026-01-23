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
RELAYS = [
    "ferdie",
    "george",
    "henry",
    "iris",
    "jack",
    "paul",
    "quinn",
    "rita",
    "sam",
    "tom"
]
TARGET_START_INDEX = 4
TARGET_END_INDEX = 10
FUNDING_START_INDEX = 1
FUNDING_END_INDEX = 3
TOKEN_TYPE = "0000000000000000000000000000000000000000000000000000000000000000"
DB_PATH = "toolkit.db"
NODE_URL = "ws://localhost:9944" #"ws://ferdie.node.sc.iog.io:9944"
FUNDING_AMOUNT = 3000000

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

def fund_address(address, funding_seed, node_url, cwd=None):
    """Funds the given address using the source seed."""

    cmd = [
        TOOLKIT_CMD, "generate-txs", "single-tx",
        "--source-seed", funding_seed,
        "--src-url", node_url,
        "--unshielded-amount", str(AMOUNT),
        "--unshielded-token-type", TOKEN_TYPE,
        "--destination-address", address,
        "--dest-url", node_url
    ]

    # Run the command (output is captured but we assume success if no error raised)
    run_command(cmd, cwd=cwd)

def process_chunk(chunk_start, chunk_end, funding_seeds, node_url):
    try:
        relay_name = node_url.split('//')[1].split('.')[0]
    except IndexError:
        relay_name = "unknown"
    print(f"🚀 Starting chunk {chunk_start}-{chunk_end} on {relay_name}")

    with tempfile.TemporaryDirectory() as temp_dir:
        # Copy toolkit.db to temp_dir to avoid locking
        if os.path.exists(DB_PATH):
            shutil.copy(DB_PATH, os.path.join(temp_dir, "toolkit.db"))

        for i, seed in zip(range(chunk_start, chunk_end + 1), funding_seeds):
            try:
                print(f"[Chunk {seed[-4:]}] Generating wallet {i}...", end=" ", flush=True)
                addr = get_wallet_address(i, cwd=temp_dir)
                print(f"✅ {addr}")

                print(f"[Chunk {seed[-4:]}] Funding {addr}...", end=" ", flush=True)
                fund_address(addr, seed, node_url, cwd=temp_dir)
                print("✅ Sent")

                # Wait a bit between transactions to ensure nonce propagation
                time.sleep(2)
            except Exception as e:
                print(f"\n❌ Failed processing index {i}: {e}")

def main():
    os.environ["MN_DONT_WATCH_PROGRESS"] = "false"
    parser = argparse.ArgumentParser(description="Fund wallets.")
    parser.add_argument("--start", type=int, default=TARGET_START_INDEX, help="Starting seed to be funded")
    parser.add_argument("--end", type=int, default=TARGET_END_INDEX, help="Ending seed to be funded")
    parser.add_argument("--funding-start", type=int, default=FUNDING_START_INDEX, help="Starting funding seed index")
    parser.add_argument("--funding-end", type=int, default=FUNDING_END_INDEX, help="Ending funding seed index")
    parser.add_argument("--night-amount", type=int, default=FUNDING_AMOUNT, help="Amount of NIGHT tokens to fund")
    parser.add_argument("--node-url", type=str, default=NODE_URL, help="Node URL. 'ferdie' will be replaced by relay names if present.")
    args = parser.parse_args()

    global AMOUNT
    AMOUNT = args.night_amount * 10**6

    start_index = args.start
    end_index = args.end
    funding_start = args.funding_start
    funding_end = args.funding_end
    source_seeds = [f"{i:064}" for i in range(funding_start, funding_end + 1)]

    print("🚀 Starting wallet creation and funding script...")

    total_wallets = end_index - start_index + 1
    # Determine the number of workers based on the minimum of available resources
    cpu_count = os.cpu_count() or 1
    max_threads = max(1, int(cpu_count * 0.9))
    num_workers = min(len(source_seeds), max_threads)
    print(f"ℹ️  Using {num_workers} threads for execution.")

    if num_workers == 0:
        print("❌ No funding seeds or relays configured. Exiting.")
        sys.exit(1)
    chunk_size = math.ceil(total_wallets / num_workers)

    start_time = time.time()

    with concurrent.futures.ThreadPoolExecutor(max_workers=num_workers) as executor:
        futures = []
        for i in range(num_workers):
            chunk_start = start_index + i * chunk_size
            chunk_end = min(start_index + (i + 1) * chunk_size - 1, end_index)

            if chunk_start > chunk_end:
                break

            # Round-robin selection of relay node
            relay_name = RELAYS[i % len(RELAYS)]
            if "ferdie" in args.node_url:
                node_url = args.node_url.replace("ferdie", relay_name)
            else:
                node_url = args.node_url

            # Calculate which seeds belong to this chunk
            # We use modulo to cycle seeds if there are fewer seeds than wallets
            chunk_len = chunk_end - chunk_start + 1
            chunk_seeds = [source_seeds[(chunk_start - start_index + k) % len(source_seeds)] for k in range(chunk_len)]

            futures.append(executor.submit(process_chunk, chunk_start, chunk_end, chunk_seeds, node_url))

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
