#!/usr/bin/env python3
"""
All-in-one script to fetch block sizes from a Substrate node and generate visualizations.
"""

import argparse
import subprocess
import sys
from pathlib import Path


def main():
    parser = argparse.ArgumentParser(
        description="Fetch and visualize block sizes from a Substrate node",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog="""
Examples:
  # Analyze latest 100 blocks from local node
  %(prog)s --url ws://127.0.0.1:9944 --latest-n 100
  
  # Analyze specific block range
  %(prog)s --url ws://127.0.0.1:9944 --start-block 1000 --end-block 2000
  
  # Specify output directory
  %(prog)s --url ws://127.0.0.1:9944 --latest-n 50 --output-dir ./my_analysis
        """
    )
    
    parser.add_argument("--url", 
                       default="ws://127.0.0.1:9944",
                       help="WebSocket URL of the Substrate node (default: ws://127.0.0.1:9944)")
    
    parser.add_argument("--start-block", 
                       type=int,
                       help="Starting block number")
    
    parser.add_argument("--end-block", 
                       type=int,
                       help="Ending block number")
    
    parser.add_argument("--latest-n", 
                       type=int,
                       help="Fetch the latest N blocks")
    
    parser.add_argument("--output-dir", 
                       help="Output directory for data and graphs (default: auto-generated based on block range)")
    
    parser.add_argument("--skip-fetch", 
                       action="store_true",
                       help="Skip fetching and use existing CSV file")
    
    parser.add_argument("--csv-file", 
                       help="Path to existing CSV file (required if --skip-fetch is used)")
    
    args = parser.parse_args()
    
    script_dir = Path(__file__).parent.absolute()
    
    # Determine output directory
    if args.output_dir:
        output_dir = Path(args.output_dir)
    else:
        # Auto-generate directory name
        if args.latest_n:
            dir_name = f"block_size_analysis_latest_{args.latest_n}"
        elif args.start_block is not None and args.end_block is not None:
            dir_name = f"block_size_analysis_{args.start_block}_to_{args.end_block}"
        else:
            dir_name = "block_size_analysis"
        output_dir = script_dir / dir_name
    
    output_dir.mkdir(parents=True, exist_ok=True)
    csv_file = output_dir / "block_sizes.csv"
    
    print("=" * 60)
    print("BLOCK SIZE ANALYSIS")
    print("=" * 60)
    print()
    
    # Step 1: Fetch block data (unless skipped)
    if not args.skip_fetch:
        print("STEP 1: Fetching block size data...")
        print("-" * 60)
        
        fetch_cmd = [
            sys.executable,
            str(script_dir / "fetch_block_sizes.py"),
            "--url", args.url,
            "--output", str(csv_file)
        ]
        
        if args.latest_n:
            fetch_cmd.extend(["--latest-n", str(args.latest_n)])
        elif args.start_block is not None and args.end_block is not None:
            fetch_cmd.extend(["--start-block", str(args.start_block)])
            fetch_cmd.extend(["--end-block", str(args.end_block)])
        else:
            print("Error: Either --latest-n or both --start-block and --end-block must be specified")
            sys.exit(1)
        
        try:
            subprocess.run(fetch_cmd, check=True)
        except subprocess.CalledProcessError as e:
            print(f"\nError fetching block data: {e}")
            sys.exit(1)
        
        print()
    else:
        if not args.csv_file:
            print("Error: --csv-file is required when using --skip-fetch")
            sys.exit(1)
        csv_file = Path(args.csv_file)
        if not csv_file.exists():
            print(f"Error: CSV file not found: {csv_file}")
            sys.exit(1)
        print(f"Using existing CSV file: {csv_file}")
        print()
    
    # Step 2: Generate visualizations
    print("STEP 2: Generating visualizations...")
    print("-" * 60)
    
    plot_cmd = [
        sys.executable,
        str(script_dir / "plot_block_sizes.py"),
        "--input", str(csv_file),
        "--output-dir", str(output_dir)
    ]
    
    try:
        subprocess.run(plot_cmd, check=True)
    except subprocess.CalledProcessError as e:
        print(f"\nError generating visualizations: {e}")
        sys.exit(1)
    
    print()
    print("=" * 60)
    print("ANALYSIS COMPLETE")
    print("=" * 60)
    print(f"\nAll outputs saved to: {output_dir}")
    print(f"\nGenerated files:")
    print(f"  • {csv_file.name} - Raw block size data")
    print(f"  • block_sizes_over_time.png - Line chart of block sizes")
    print(f"  • block_size_distribution.png - Histogram of block sizes")
    print(f"  • block_size_vs_extrinsics.png - Scatter plot")
    print(f"  • block_size_analysis_dashboard.png - Combined dashboard")
    print(f"  • block_size_report.txt - Statistical report")
    print(f"\nTo view the dashboard:")
    print(f"  open {output_dir / 'block_size_analysis_dashboard.png'}")


if __name__ == "__main__":
    main()
