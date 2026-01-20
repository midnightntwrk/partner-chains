#!/usr/bin/env python3
"""
Mempool Benchmark Runner

This script automates the process of:
1. Downloading Ferdie's logs from Grafana/Loki
2. Extracting mempool metrics
3. Analyzing the metrics with configurable time windows
"""

import argparse
import json
import subprocess
import sys
from pathlib import Path
from datetime import datetime


def run_command(cmd, description, cwd=None):
    """Run a command and handle errors."""
    print(f"\n{'='*60}")
    print(f"{description}")
    print(f"{'='*60}")
    print(f"Command: {' '.join(cmd)}\n")

    try:
        result = subprocess.run(
            cmd,
            cwd=cwd,
            capture_output=True,
            text=True,
            check=True
        )
        print(result.stdout)
        if result.stderr:
            print("STDERR:", result.stderr)
        return True
    except subprocess.CalledProcessError as e:
        print(f"ERROR: {description} failed!")
        print(f"Exit code: {e.returncode}")
        print(f"STDOUT: {e.stdout}")
        print(f"STDERR: {e.stderr}")
        return False
    except Exception as e:
        print(f"ERROR: Unexpected error during {description}: {e}")
        return False


def main():
    parser = argparse.ArgumentParser(
        description="Run mempool benchmarking: download logs, extract, and analyze metrics.",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog="""
Examples:
  # Analyze Ferdie (relay node, default)
  python3 run_mempool_benchmark.py \\
    --config ../../secrets/substrate/performance/performance.json \\
    --from-time "2026-01-08T10:00:00Z" \\
    --to-time "2026-01-08T10:10:00Z"

  # Analyze Charlie (validator node)
  python3 run_mempool_benchmark.py \\
    --node charlie \\
    --config ../../secrets/substrate/performance/performance.json \\
    --from-time "2026-01-08T10:00:00Z" \\
    --to-time "2026-01-08T10:10:00Z"
        """
    )

    # Node selection
    parser.add_argument("--node", default="ferdie", 
                       help="Node to download logs from (default: ferdie)")
    
    # Download options
    parser.add_argument("--config", 
                       default="../../../secrets/substrate/performance/performance.json",
                       help="Path to encrypted config file with Grafana credentials (default: ../../../secrets/substrate/performance/performance.json)")
    parser.add_argument("--url", help="Loki API URL (overrides config file)")
    parser.add_argument("--header", action='append', help="Custom header 'Key: Value' (can be used multiple times)")

    # Time range
    parser.add_argument("--time-range", help='Time range as JSON, e.g., \'{"from":"2026-01-20 10:34:25","to":"2026-01-20 11:34:25"}\'')
    parser.add_argument("--from-time", dest="start_time",
                       help="Start time in ISO 8601 format or YYYY-MM-DD HH:MM:SS (e.g., '2026-01-08T10:00:00Z')")
    parser.add_argument("--to-time", dest="end_time",
                       help="End time in ISO 8601 format or YYYY-MM-DD HH:MM:SS (e.g., '2026-01-08T10:10:00Z')")

    # Analysis options
    parser.add_argument("--window", type=int, default=None,
                       help="Time window in milliseconds for analysis (default: auto-calculated from time range)")
    parser.add_argument("--output-dir", default=".",
                       help="Output directory for all files (default: current directory)")

    # Skip steps
    parser.add_argument("--skip-download", action="store_true",
                       help="Skip log download (use existing ferdie.txt)")
    parser.add_argument("--skip-extract", action="store_true",
                       help="Skip extraction (use existing mempool_report.txt)")

    args = parser.parse_args()
    
    # Parse time range from JSON if provided
    if args.time_range:
        try:
            time_data = json.loads(args.time_range)
            args.start_time = time_data.get('from')
            args.end_time = time_data.get('to')
            if not args.start_time or not args.end_time:
                print("Error: time-range JSON must contain 'from' and 'to' fields")
                sys.exit(1)
        except json.JSONDecodeError as e:
            print(f"Error: Invalid JSON in --time-range: {e}")
            sys.exit(1)
    
    # Validate that we have time range (unless skipping download)
    if not args.skip_download and (not args.start_time or not args.end_time):
        print("Error: Either --time-range or both --from-time and --to-time must be provided")
        sys.exit(1)

    # Resolve output directory
    output_dir = Path(args.output_dir).resolve()
    output_dir.mkdir(parents=True, exist_ok=True)

    script_dir = Path(__file__).parent.resolve()

    # Calculate window if not provided
    if args.window is None:
        # Parse time range to calculate total duration
        # Handle both space-separated and T-separated formats
        start_time_normalized = args.start_time.replace(' ', 'T').replace('Z', '+00:00')
        end_time_normalized = args.end_time.replace(' ', 'T').replace('Z', '+00:00')
        start = datetime.fromisoformat(start_time_normalized)
        end = datetime.fromisoformat(end_time_normalized)
        duration_seconds = (end - start).total_seconds()
        # Use 1 second window as default for all durations
        args.window = 1000
        print(f"\nAuto-calculated window: {args.window}ms for {duration_seconds:.0f}s duration")

    print(f"\n{'#'*60}")
    print(f"# MEMPOOL BENCHMARK RUNNER")
    print(f"{'#'*60}")
    print(f"Node: {args.node}")
    print(f"Time range: {args.start_time} to {args.end_time}")
    print(f"Analysis window: {args.window}ms")
    print(f"Output directory: {output_dir}")
    print(f"Script directory: {script_dir}")

    # Step 1: Download logs
    if not args.skip_download:
        download_cmd = [
            sys.executable,
            str(script_dir.parent / "download_logs.py"),
            "--node", args.node,
            "--from-time", args.start_time,
            "--to-time", args.end_time
        ]

        # Only add output-dir if specified by user
        if args.output_dir != ".":
            download_cmd.extend(["--output-dir", str(output_dir)])

        if args.config:
            download_cmd.extend(["--config", args.config])
        if args.url:
            download_cmd.extend(["--url", args.url])
        if args.header:
            for header in args.header:
                download_cmd.extend(["--header", header])

        if not run_command(download_cmd, f"Step 1: Downloading {args.node}'s logs"):
            sys.exit(1)

        # Construct expected directory name based on date range
        try:
            # Handle both space-separated and T-separated formats
            start_time_normalized = args.start_time.replace(' ', 'T').replace('Z', '+00:00')
            end_time_normalized = args.end_time.replace(' ', 'T').replace('Z', '+00:00')
            start_dt = datetime.fromisoformat(start_time_normalized)
            end_dt = datetime.fromisoformat(end_time_normalized)
            start_str = start_dt.strftime("%Y-%m-%d_%H-%M-%S")
            end_str = end_dt.strftime("%Y-%m-%d_%H-%M-%S")
            date_range_folder = f"from_{start_str}_to_{end_str}"
        except Exception:
            # Fallback to simple names
            start_str = args.start_time.replace(':', '-').replace('T', '_').replace(' ', '_')
            end_str = args.end_time.replace(':', '-').replace('T', '_').replace(' ', '_')
            date_range_folder = f"from_{start_str}_to_{end_str}"
        
        # Determine base path
        if args.output_dir != ".":
            base_path = output_dir
        else:
            # download_logs.py will use its own script_dir/logs
            base_path = script_dir.parent / "logs"
        
        log_dir = base_path / date_range_folder
        
        if not log_dir.exists():
            print(f"ERROR: Expected log directory does not exist: {log_dir}")
            sys.exit(1)
            
        print(f"\nUsing log directory: {log_dir}")
        
        # Check that node log file exists in the directory
        node_file = log_dir / f"{args.node}.txt"
        if not node_file.exists():
            print(f"ERROR: {node_file} not found in downloaded directory!")
            sys.exit(1)
    else:
        print("\nStep 1: SKIPPED (using existing logs)")
        log_dir = output_dir
        node_file = log_dir / f"{args.node}.txt"
        if not node_file.exists():
            print(f"ERROR: {node_file} not found!")
            sys.exit(1)

    # Step 2: Extract metrics
    if not args.skip_extract:
        extract_cmd = [
            sys.executable,
            str(script_dir / "extractor.py"),
            args.node
        ]

        if not run_command(extract_cmd, "Step 2: Extracting mempool metrics", cwd=log_dir):
            sys.exit(1)

        # Check if report was created
        report_file = log_dir / "mempool_report.txt"
        if not report_file.exists():
            print("ERROR: mempool_report.txt was not created!")
            sys.exit(1)
        print(f"\n✓ Created: {report_file}")
    else:
        print("\nStep 2: SKIPPED (using existing mempool_report.txt)")
        report_file = log_dir / "mempool_report.txt"
        if not report_file.exists():
            print(f"ERROR: {report_file} not found!")
            sys.exit(1)

    # Step 3: Analyze metrics
    timestamp = datetime.now().strftime("%Y%m%d_%H%M%S")
    analysis_file = log_dir / f"mempool_analysis_{timestamp}.txt"

    analyze_cmd = [
        sys.executable,
        str(script_dir / "analyzer.py"),
        "mempool_report.txt",
        str(analysis_file.name),
        str(args.window)
    ]

    if not run_command(analyze_cmd, "Step 3: Analyzing metrics", cwd=log_dir):
        sys.exit(1)

    # Print summary
    print(f"\n{'='*60}")
    print("SUCCESS! Mempool benchmark complete.")
    print(f"{'='*60}")
    print(f"\nOutput files in: {log_dir}")
    print(f"  - {args.node}.txt: Downloaded logs")
    print(f"  - mempool_report.txt: Extracted time-series data")
    print(f"  - mempool_events.csv: Raw event data (CSV)")
    print(f"  - {analysis_file.name}: Analysis and statistics")
    print(f"  - {analysis_file.name.rsplit('.', 1)[0]}_timeseries.csv: Time-series data (CSV)")
    print(f"\nTo view analysis:")
    print(f"  cat {analysis_file}")
    print()


if __name__ == "__main__":
    main()
