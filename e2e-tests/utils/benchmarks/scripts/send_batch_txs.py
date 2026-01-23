import subprocess
import glob
import os
import re
import sys
import time
import concurrent.futures
import tempfile
import json
import argparse
import random

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
NODE_URL = "ws://ferdie.node.sc.iog.io:9944" # "ws://localhost:9944"
MAX_RETRIES = 5

def submit_single_tx(i, tx_file, total_files, toolkit_path, node_url_pattern, max_retries, verbose=False, max_workers=1):
    # Ensure absolute path for the source file since we change CWD
    abs_tx_file = os.path.abspath(tx_file)

    start_relay_idx = i % len(RELAYS)
    num_attempts = min(len(RELAYS), max_retries)

    for r_offset in range(num_attempts):
        relay_idx = (start_relay_idx + r_offset) % len(RELAYS)
        relay_name = RELAYS[relay_idx]
        if "ferdie" in node_url_pattern:
            dest_url = node_url_pattern.replace("ferdie", relay_name)
        else:
            dest_url = node_url_pattern

        cmd = [
            toolkit_path, "generate-txs", "send",
            "--src-file", abs_tx_file,
            "--dest-url", dest_url
        ]

        if verbose:
            print(f"[{i}/{total_files}] Running: {' '.join(cmd)}")

        try:
            with tempfile.TemporaryDirectory() as temp_dir:
                exec_start = time.time()
                result = subprocess.run(
                    cmd, capture_output=True, text=True, check=True, cwd=temp_dir
                )
                if verbose:
                    if result.stdout:
                        print(f"[{i}/{total_files}] STDOUT: {result.stdout.strip()}")
                    if result.stderr:
                        print(f"[{i}/{total_files}] STDERR: {result.stderr.strip()}")
                exec_time = time.time() - exec_start

            chain_latency = 0.0
            sent_ts = 0
            finalized_ts = 0

            if result.stderr:
                print(f"⚠️  {tx_file}: {result.stderr}")
                try:
                    for line in result.stderr.splitlines():
                        if '"message":"SENT"' in line:
                            sent_ts = json.loads(line[line.find("{"):])["timestamp"]
                        elif '"message":"FINALIZED"' in line:
                            finalized_ts = json.loads(line[line.find("{"):])["timestamp"]
                    if sent_ts and finalized_ts:
                        chain_latency = (finalized_ts - sent_ts) / 1000.0
                except Exception:
                    pass

            if "RPC error: User error: Invalid Transaction (1010)" in result.stdout or \
               "RPC error: User error: Invalid Transaction (1010)" in result.stderr:
                print(f"❌ [{i}/{total_files}] Failed to send {tx_file} to {relay_name} (Invalid Transaction 1010) [Exec: {exec_time:.4f}s]")
                return False

            if "RPC error: User error: Transaction is temporarily banned (1012)" in result.stdout or \
               "RPC error: User error: Transaction is temporarily banned (1012)" in result.stderr:
                print(f"⛔ [{i}/{total_files}] Failed to send {tx_file} to {relay_name} (Temporarily Banned 1012) [Exec: {exec_time:.4f}s]")
                return "Banned"

            print(f"✅ [{i}/{total_files}] Sent {tx_file} to {relay_name} [Chain Latency: {chain_latency:.2f}s, Exec: {exec_time:.4f}s]")
            if max_workers > 1:
                time.sleep(random.uniform(0.05, 0.5))
            return True

        except subprocess.CalledProcessError as e:
            if r_offset == num_attempts - 1:
                print(f"\n❌ Failed to submit {tx_file} to {relay_name}!")
                print("Error Output:", e.stderr)
                return False
            else:
                print(f"⚠️  Failed to submit {tx_file} to {dest_url}, trying next node...")
                time.sleep(0.5)

def submit_transactions(toolkit_path="midnight-node-toolkit"):
    # Disable watching for txs to finalize
    os.environ["MN_DONT_WATCH_PROGRESS"] = "true"

    parser = argparse.ArgumentParser(description="Submit batch transactions.")
    parser.add_argument("--start", type=int, help="Start index")
    parser.add_argument("--end", type=int, help="End index")
    parser.add_argument("--verbose", action="store_true", help="Enable verbose output")
    parser.add_argument("--workers", type=int, help="Number of concurrent workers")
    parser.add_argument("--node-url", type=str, default=NODE_URL, help="Node URL. 'ferdie' will be replaced by relay names if present.")
    parser.add_argument("--max-retries", type=int, default=MAX_RETRIES, help="Maximum number of attempts per transaction.")
    args = parser.parse_args()

    start_time = time.time()
    # 1. Find all matching files
    all_files = glob.glob(os.path.join("txs", "tx_*.mn"))

    files = []
    if args.start is not None and args.end is not None:
        for f in all_files:
            try:
                basename = os.path.basename(f)
                index = int(os.path.splitext(basename)[0].split('_')[-1])
                if args.start <= index <= args.end:
                    files.append(f)
            except (ValueError, IndexError):
                continue
    else:
        files = all_files

    if not files:
        msg = f" in range {args.start}-{args.end}" if args.start is not None else ""
        print(f"❌ No files found matching 'tx_*.mn'{msg}")
        sys.exit(1)

    print(f"🚀 Found {len(files)} transaction files to submit.")

    if args.workers:
        max_workers = args.workers
    else:
        max_workers = min(os.cpu_count() or 1, len(files))
    print(f"ℹ️  Using {max_workers} threads for execution.")

    results = []
    with concurrent.futures.ThreadPoolExecutor(max_workers=max_workers) as executor:
        futures = [executor.submit(submit_single_tx, i, tx_file, len(files), toolkit_path, args.node_url, args.max_retries, verbose=args.verbose, max_workers=max_workers) for i, tx_file in enumerate(files, 1)]
        for future in concurrent.futures.as_completed(futures):
            results.append(future.result())

    end_time = time.time()
    print("\n🎉 Batch submission complete.")
    print(f"Valid: {results.count(True)}, Invalid: {results.count(False)}, Temporarily Banned: {results.count('Banned')}")
    print(f"⏱️ Total execution time: {end_time - start_time:.2f} seconds")

if __name__ == "__main__":
    submit_transactions()
