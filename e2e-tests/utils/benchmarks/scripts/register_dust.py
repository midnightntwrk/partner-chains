import subprocess
import time
import sys
import concurrent.futures
import math
import shutil
import tempfile
import os
import argparse


# Configuration
TARGET_START_INDEX = 11
TARGET_END_INDEX = 499
FUNDING_START_INDEX = 1
FUNDING_END_INDEX = 3
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
TOOLKIT_PATH = "midnight-node-toolkit"
DB_PATH = "toolkit.db"

def register_chunk(chunk_start, chunk_end, funding_seed, node_url, toolkit_path):
    try:
        relay_name = node_url.split('//')[1].split('.')[0]
    except IndexError:
        relay_name = "unknown"
    print(f"🚀 Starting chunk {chunk_start}-{chunk_end} on {relay_name} with funding seed ...{funding_seed[-2:]}")

    with tempfile.TemporaryDirectory() as temp_dir:
        if os.path.exists(DB_PATH):
            shutil.copy(DB_PATH, os.path.join(temp_dir, "toolkit.db"))

        for i in range(chunk_start, chunk_end + 1):
            # Format the seed: Pad '20' to '000...00020' (64 chars total)
            # We treat '20' as the literal suffix the user requested
            wallet_seed = f"{i:064}"

            print(f"[Chunk {funding_seed[-2:]}] Registering dust for seed ...{i}...")

            cmd = [
                toolkit_path, "generate-txs",
                "--src-url", node_url,
                "--dest-url", node_url,
                "register-dust-address",
                "--wallet-seed", wallet_seed,
                "--funding-seed", funding_seed
            ]

            try:
                # Run the command
                result = subprocess.run(
                    cmd,
                    capture_output=True,
                    text=True,
                    check=True,
                    cwd=temp_dir
                )
                print(f"✅ Success (Seed ...{i})")
                # print(result.stdout) # Uncomment if you want to see the tx hash

            except subprocess.CalledProcessError as e:
                print(f"\n❌ Failed to register seed ...{i}!")
                if "len (is 0)" in e.stderr:
                    print("💡 Hint: The funding wallet likely has no funds (0 UTXOs).")
                print("Error Output:", e.stderr)
                # We continue to the next one even if one fails

            except FileNotFoundError:
                print(f"\n❌ Error: Could not find '{toolkit_path}'.")
                sys.exit(1)

            # Wait 2 seconds between registrations to ensure the funding account's
            # previous transaction is processed/propagated (prevents nonce/dust errors)
            time.sleep(2) # Check to remove

def register_dust_addresses():
    os.environ["MN_DONT_WATCH_PROGRESS"] = "false"
    parser = argparse.ArgumentParser(description="Register dust addresses.")
    parser.add_argument("--start", type=int, default=TARGET_START_INDEX, help="Starting seed to be registered")
    parser.add_argument("--end", type=int, default=TARGET_END_INDEX, help="Ending seed to be registered")
    parser.add_argument("--funding-start", type=int, default=FUNDING_START_INDEX, help="Starting funding seed index")
    parser.add_argument("--funding-end", type=int, default=FUNDING_END_INDEX, help="Ending funding seed index")
    args = parser.parse_args()

    start_index = args.start
    end_index = args.end
    funding_start = args.funding_start
    funding_end = args.funding_end
    funding_seeds = [f"{i:064}" for i in range(funding_start, funding_end + 1)]

    print(f"🚀 Starting dust registration for seeds ending in {start_index} to {end_index}...")

    total_wallets = end_index - start_index + 1
    # Determine the number of workers based on the minimum of available resources
    num_workers = min(len(funding_seeds), os.cpu_count() or 1)
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
            node_url = f"ws://{relay_name}.node.sc.iog.io:9944"
            futures.append(executor.submit(register_chunk, chunk_start, chunk_end, funding_seeds[i], node_url, TOOLKIT_PATH))

        concurrent.futures.wait(futures)

    end_time = time.time()
    total_duration = end_time - start_time

    print("\n🎉 All registration commands completed.")
    if total_duration > 120:
        minutes = int(total_duration // 60)
        seconds = total_duration % 60
        print(f"⏱️ Total execution time for {total_wallets} wallets: {minutes} minutes and {seconds:.2f} seconds")
    else:
        print(f"⏱️ Total execution time for {total_wallets} wallets: {total_duration:.2f} seconds")
    if total_wallets > 0:
        print(f"📊 Average time per registration: {total_duration / total_wallets:.2f} seconds")

if __name__ == "__main__":
    register_dust_addresses()