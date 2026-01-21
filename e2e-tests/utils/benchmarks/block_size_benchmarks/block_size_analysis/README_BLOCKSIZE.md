# Block Size Analysis Tools

Tools for measuring and visualizing blockchain block sizes from Substrate/Polkadot nodes.

## Overview

This toolset fetches block data from a Substrate node via RPC, calculates block sizes in bytes/KB/MB, and generates comprehensive visualizations and reports.

## Installation

1. Install Python dependencies:

```bash
pip3 install -r requirements_blocksize.txt
```

Or install individually:

```bash
pip3 install substrate-interface matplotlib numpy
```

## Quick Start

### Option 1: All-in-One Script (Recommended)

Analyze the latest 100 blocks:

```bash
python3 analyze_block_sizes.py --url ws://127.0.0.1:9944 --latest-n 100
```

Analyze a specific block range:

```bash
python3 analyze_block_sizes.py --url ws://127.0.0.1:9944 --start-block 1000 --end-block 2000
```

Specify output directory:

```bash
python3 analyze_block_sizes.py --url ws://127.0.0.1:9944 --latest-n 50 --output-dir ./my_analysis
```

### Option 2: Step-by-Step

1. **Fetch block size data:**

```bash
python3 fetch_block_sizes.py --url ws://127.0.0.1:9944 --latest-n 100 --output block_sizes.csv
```

2. **Generate visualizations:**

```bash
python3 plot_block_sizes.py --input block_sizes.csv --output-dir ./output
```

## Scripts

### `analyze_block_sizes.py`

All-in-one script that fetches data and generates visualizations.

**Arguments:**
- `--url` - WebSocket URL of the node (default: `ws://127.0.0.1:9944`)
- `--latest-n` - Fetch the latest N blocks
- `--start-block` - Starting block number
- `--end-block` - Ending block number
- `--output-dir` - Output directory (default: auto-generated)
- `--skip-fetch` - Skip fetching, use existing CSV
- `--csv-file` - Path to existing CSV (if `--skip-fetch`)

### `fetch_block_sizes.py`

Fetches block data from a Substrate node and saves to CSV.

**Arguments:**
- `--url` - WebSocket URL of the node
- `--latest-n` - Fetch the latest N blocks
- `--start-block` - Starting block number
- `--end-block` - Ending block number
- `--output` - Output CSV file (default: `block_sizes.csv`)

**Output CSV Columns:**
- `block_number` - Block number
- `block_hash` - Block hash
- `size_bytes` - Block size in bytes
- `size_kb` - Block size in KB
- `size_mb` - Block size in MB
- `extrinsic_count` - Number of extrinsics in the block
- `timestamp` - Block timestamp (if available)

### `plot_block_sizes.py`

Generates visualizations from CSV data.

**Arguments:**
- `--input` - Input CSV file (required)
- `--output-dir` - Output directory for graphs (default: current directory)

**Generated Files:**
- `block_sizes_over_time.png` - Line chart showing block size over block numbers
- `block_size_distribution.png` - Histogram of block size distribution
- `block_size_vs_extrinsics.png` - Scatter plot of block size vs extrinsic count
- `block_size_analysis_dashboard.png` - Combined dashboard with all metrics
- `block_size_report.txt` - Text report with statistics

## Usage Examples

### Analyze Local Development Node

```bash
python3 analyze_block_sizes.py --url ws://127.0.0.1:9944 --latest-n 200
```

### Analyze Remote Node

```bash
python3 analyze_block_sizes.py --url wss://rpc.polkadot.io --latest-n 50
```

### Analyze Specific Historical Range

```bash
python3 analyze_block_sizes.py \
  --url ws://127.0.0.1:9944 \
  --start-block 10000 \
  --end-block 10500 \
  --output-dir ./historical_analysis
```

### Re-plot Existing Data

```bash
python3 analyze_block_sizes.py \
  --skip-fetch \
  --csv-file ./my_data/block_sizes.csv \
  --output-dir ./new_plots
```

## Output

The scripts generate:

1. **CSV Data File** - Raw block size data for further analysis
2. **Visualizations** - Multiple PNG graphs showing:
   - Block size trends over time
   - Distribution of block sizes
   - Relationship between block size and extrinsic count
   - Combined dashboard with all metrics
3. **Text Report** - Statistical summary including:
   - Average, median, min, max block sizes
   - Standard deviation
   - Total data processed
   - Extrinsic statistics
   - Top 5 largest blocks

## Integration with Jupyter Notebooks

The CSV output can be easily imported into Jupyter notebooks for custom analysis:

```python
import pandas as pd
import matplotlib.pyplot as plt

# Load data
df = pd.read_csv('block_sizes.csv')

# Custom analysis
df['size_kb'].plot(kind='line', figsize=(14, 6))
plt.xlabel('Block Index')
plt.ylabel('Block Size (KB)')
plt.title('Custom Block Size Analysis')
plt.show()
```

## Notes

- The block size calculation is based on the JSON-encoded representation of the block
- For more accurate SCALE-encoded sizes, you may need to modify `fetch_block_sizes.py`
- Timestamps may not be available for all blocks depending on the chain configuration
- Large block ranges may take some time to fetch

## Troubleshooting

### Connection Issues

If you get connection errors, verify:
- Node is running and accessible
- WebSocket endpoint is correct
- Firewall/network allows connections

### Import Errors

If you get import errors, install dependencies:

```bash
pip3 install -r requirements_blocksize.txt
```

### Memory Issues

For very large block ranges (>10,000 blocks), consider:
- Processing in smaller batches
- Increasing available memory
- Using `--skip-fetch` to regenerate plots without re-fetching

## Related Tools

This toolset complements the existing benchmarking infrastructure:
- `run_benchmark.py` - Block propagation and import time analysis
- `mempool_benchmarks/` - Mempool performance analysis
- `jupyter/` - Jupyter notebooks for interactive analysis
