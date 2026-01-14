#!/usr/bin/env python3
import subprocess
import json
import sys
import os
import time
import shutil
import tempfile
import concurrent.futures

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
START_INDEX = 20
END_INDEX = 25
DB_PATH = "toolkit.db"

def get_balance(index):
    """Gets the balance for a given seed index."""
    seed = f"{index:064}"
    
    relay_name = RELAYS[index % len(RELAYS)]
    node_url = f"ws://{relay_name}.node.sc.iog.io:9944"
    
    cmd = [
        TOOLKIT_CMD, "show-wallet",
        "--seed", seed,
        "--src-url", node_url
    ]
    
    try:
        with tempfile.TemporaryDirectory() as temp_dir:
            # Copy toolkit.db to temp_dir to avoid locking
            db_copy_start = time.time()
            if os.path.exists(DB_PATH):
                shutil.copy(DB_PATH, os.path.join(temp_dir, "toolkit.db"))
            db_copy_time = time.time() - db_copy_start

            exec_start = time.time()
            result = subprocess.run(cmd, capture_output=True, text=True, check=True, cwd=temp_dir)
            exec_time = time.time() - exec_start
        
        output = result.stdout
        
        # Mimic sed -n '/^{/,$p': Find lines starting from the first one that begins with '{'
        lines = output.splitlines()
        json_lines = []
        capture = False
        for line in lines:
            if line.strip().startswith('{'):
                capture = True
            if capture:
                json_lines.append(line)
        
        if not json_lines:
            print(f"⚠️  Seed {index}: No JSON output found.")
            return 0
            
        json_str = "\n".join(json_lines)
        data = json.loads(json_str)
        
        # Mimic jq '.utxos[]?.value' | jq -s 'add'
        utxos = data.get("utxos", [])
        if not utxos:
            return 0
            
        total_balance = sum(int(utxo.get("value", 0)) for utxo in utxos)
        print(f"Seed {index}: {total_balance} [DB Copy: {db_copy_time:.4f}s, Exec: {exec_time:.4f}s]")
        return total_balance

    except subprocess.CalledProcessError as e:
        print(f"❌ Error getting balance for seed {index}: {e.stderr.strip()}")
        return 0
    except json.JSONDecodeError:
        print(f"❌ Failed to parse JSON for seed {index}")
        return 0
    except Exception as e:
        print(f"❌ Unexpected error for seed {index}: {e}")
        return 0

def main():
    global DB_PATH
    if not os.path.exists(DB_PATH):
        print(f"⚠️  Warning: '{DB_PATH}' not found in current directory.")
        user_input = input("Please enter the full path to toolkit.db: ").strip()
        if not user_input:
            print("❌ No path provided. Exiting.")
            sys.exit(1)
        
        DB_PATH = user_input
        if not os.path.exists(DB_PATH):
            print(f"❌ Error: File '{DB_PATH}' not found.")
            sys.exit(1)

    start_time = time.time()
    print(f"🚀 Checking balances for seeds {START_INDEX} to {END_INDEX} across {len(RELAYS)} nodes...")
    
    num_seeds = END_INDEX - START_INDEX + 1
    max_workers = min(os.cpu_count() or 1, num_seeds)
    
    total_sum = 0
    with concurrent.futures.ThreadPoolExecutor(max_workers=max_workers) as executor:
        futures = [executor.submit(get_balance, i) for i in range(START_INDEX, END_INDEX + 1)]
        for future in concurrent.futures.as_completed(futures):
            total_sum += future.result()

    end_time = time.time()
    print(f"\n💰 Total Balance: {total_sum}")
    print(f"⏱️ Total execution time: {end_time - start_time:.2f} seconds")

if __name__ == "__main__":
    main()
