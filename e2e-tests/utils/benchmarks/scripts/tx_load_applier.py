import argparse
import time
import subprocess
import os
import shutil
import sys

def main():
    parser = argparse.ArgumentParser(description="Apply transaction load by generating and submitting batches continuously.")
    parser.add_argument("--loading-time", type=int, default=3600, help="Duration in seconds to run the load test (default: 1 hour).")
    parser.add_argument("--batch-size", type=int, default=10, help="Number of transactions to submit per batch (default: 10).")
    parser.add_argument("--batch-delay", type=float, default=6.0, help="Delay in seconds between batches (default: 6.0).")
    parser.add_argument("-s", "--start-seed", type=int, default=10, help="Starting seed index (default: 10).")
    parser.add_argument("-e","--end-seed", type=int, default=1000, help="Ending seed index of the funded wallets (default: 1000).")
    parser.add_argument("--seeds-per-iteration", type=int, default=100, help="Number of seeds to process per iteration (default: 100).")
    parser.add_argument("--verbose", action="store_true", help="Enable verbose output.")

    args = parser.parse_args()

    print(f"🚀 Starting Load Applier")
    print(f"   Duration: {args.loading_time}s")
    print(f"   Batch Size: {args.batch_size}")
    print(f"   Batch Delay: {args.batch_delay}s")
    print(f"   Seed Range: {args.start_seed}-{args.end_seed}")

    start_time = time.time()
    current_seed = args.start_seed
    iteration = 0

    submission_process = None
    previous_tx_dir = None

    # Ensure base txs dir exists
    if not os.path.exists("txs"):
        os.makedirs("txs")

    try:
        while time.time() - start_time < args.loading_time:
            if current_seed > args.end_seed:
                print(f"🔄 Reached end of funded range ({args.end_seed}). Wrapping around to start ({args.start_seed}).")
                current_seed = args.start_seed

            end_seed = current_seed + args.seeds_per_iteration - 1
            if end_seed > args.end_seed:
                end_seed = args.end_seed

            print(f"\n[Iteration {iteration}] Generating txs for seeds {current_seed}-{end_seed}...")

            # Prepare directory for this batch
            batch_tx_dir = f"txs_batch_{iteration}"
            if os.path.exists(batch_tx_dir):
                shutil.rmtree(batch_tx_dir)
            os.makedirs(batch_tx_dir)

            # Clean 'txs' directory to ensure we only get new files
            for f in os.listdir("txs"):
                if f.startswith("tx_") and f.endswith(".mn"):
                    os.remove(os.path.join("txs", f))

            # Call generate_txs_round_robin.py
            gen_cmd = [
                "python3", "generate_txs_round_robin.py",
                "--dest-start", str(current_seed),
                "--dest-end", str(end_seed)
            ]
            if args.verbose:
                gen_cmd.append("--verbose")

            # We wait for generation to complete
            subprocess.run(gen_cmd, check=True)

            # Move files to batch directory
            files_moved = 0
            for f in os.listdir("txs"):
                if f.startswith("tx_") and f.endswith(".mn"):
                    shutil.move(os.path.join("txs", f), os.path.join(batch_tx_dir, f))
                    files_moved += 1

            print(f"✅ Generated {files_moved} transactions in {batch_tx_dir}")

            # Wait for previous submission to finish
            if submission_process:
                if submission_process.poll() is None:
                    print("⏳ Waiting for previous submission batch to complete...")
                    submission_process.wait()
                    print("✅ Previous submission completed.")
                else:
                    print("⚠️  Warning: Previous submission finished before generation. Network might have been idle.")

                # Cleanup previous directory
                if previous_tx_dir and os.path.exists(previous_tx_dir):
                    shutil.rmtree(previous_tx_dir)

            if files_moved > 0:
                # Start submission
                print(f"🚀 Starting submission for {batch_tx_dir}")
                submit_cmd = [
                    "python3", "send_batch_txs.py",
                    "--tx-dir", batch_tx_dir,
                    "--batch-size", str(args.batch_size),
                    "--batch-delay", str(args.batch_delay)
                ]
                if args.verbose:
                    submit_cmd.append("--verbose")

                # Run in background
                submission_process = subprocess.Popen(submit_cmd)
                previous_tx_dir = batch_tx_dir
            else:
                print("⚠️  No transactions generated. Skipping submission.")
                submission_process = None
                previous_tx_dir = None

            current_seed = end_seed + 1
            iteration += 1

    except KeyboardInterrupt:
        print("\n🛑 Stopping load applier...")
    finally:
        if submission_process and submission_process.poll() is None:
            print("⏳ Waiting for final submission to complete...")
            submission_process.wait()

        # Cleanup any remaining batch dirs
        for d in os.listdir("."):
            if d.startswith("txs_batch_"):
                shutil.rmtree(d)

        print("🎉 Load test finished.")

if __name__ == "__main__":
    main()
