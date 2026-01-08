#!/usr/bin/env python3

import sys
from dataclasses import dataclass
from datetime import datetime, timedelta
from typing import Dict, List, Tuple

import pandas as pd


@dataclass
class MempoolPoint:
    timestamp: datetime
    node: str
    ready: int
    future: int
    mempool_len: int | None
    submitted_count: int | None
    validated_count: int | None
    revalidated: int | None
    pruned_count: int | None
    reverified_txs: int | None


TS_FMT = "%Y-%m-%d %H:%M:%S.%f"


def parse_report(path: str) -> List[MempoolPoint]:
    points: List[MempoolPoint] = []
    with open(path, "r") as f:
        lines = f.readlines()
    for line in lines:
        line = line.strip()
        if not line or line.startswith("=") or line.startswith("-") or line.startswith("Time range:") or line.startswith("Total events:"):
            continue
        try:
            # Expected columns: Timestamp Node Ready Future MemLen Submit Valid Reval Pruned Reverif
            parts = [p for p in line.split(" ") if p]
            # The formatted line uses aligned spacing; rebuild by slices
            # Timestamp is first two tokens (date + time.millis)
            ts = " ".join(parts[0:2])
            node = parts[2]
            ready = parts[3]
            future = parts[4]
            mempool_len = parts[5]
            submitted = parts[6]
            validated = parts[7] if len(parts) > 7 else "N/A"
            revalidated = parts[8] if len(parts) > 8 else "N/A"
            pruned = parts[9] if len(parts) > 9 else "N/A"
            reverified = parts[10] if len(parts) > 10 else "N/A"

            # Normalize values (N/A -> None)
            def parse_int(v: str):
                return None if v == "N/A" else int(v)

            points.append(
                MempoolPoint(
                    timestamp=datetime.strptime(ts, TS_FMT),
                    node=node,
                    ready=parse_int(ready),
                    future=parse_int(future),
                    mempool_len=parse_int(mempool_len),
                    submitted_count=parse_int(submitted),
                    validated_count=parse_int(validated),
                    revalidated=parse_int(revalidated),
                    pruned_count=parse_int(pruned),
                    reverified_txs=parse_int(reverified),
                )
            )
        except Exception:
            # Ignore non-data lines
            continue
    return points


def to_dataframe(points: List[MempoolPoint]) -> pd.DataFrame:
    rows = []
    for p in points:
        rows.append({
            "timestamp": p.timestamp,
            "node": p.node,
            "ready": p.ready,
            "future": p.future,
            "mempool_len": p.mempool_len,
            "submitted": p.submitted_count,
            "validated": p.validated_count,
            "revalidated": p.revalidated,
            "pruned": p.pruned_count,
            "reverified": p.reverified_txs,
        })
    df = pd.DataFrame(rows)
    df.sort_values(["node", "timestamp"], inplace=True)
    return df


def resample_metrics(df: pd.DataFrame, window_ms: int) -> pd.DataFrame:
    # Resample per node
    all_nodes = []
    for node, group in df.groupby("node"):
        g = group.set_index("timestamp").copy()
        # Forward fill counts to build step-series of ready/future. Use last observed value.
        for col in ["ready", "future", "mempool_len"]:
            if col in g:
                g[col] = g[col].astype("Float64")
                g[col] = g[col].ffill()
        # submitted is count in event; treat as delta events per log line
        # Build rate by counting events per window
        event_counts = g["submitted"].fillna(0)
        event_counts[event_counts > 0] = 1
        submitted_rate = event_counts.resample(f"{window_ms}ms").sum() * (1000.0 / window_ms)
        # Resample ready/future via last
        ready = g["ready"].resample(f"{window_ms}ms").last()
        future = g["future"].resample(f"{window_ms}ms").last()
        mempool_len = g["mempool_len"].resample(f"{window_ms}ms").last()
        out = pd.DataFrame({
            "ready": ready,
            "future": future,
            "mempool_len": mempool_len,
            "admission_tps": submitted_rate,
        })
        out["node"] = node
        all_nodes.append(out.reset_index())
    return pd.concat(all_nodes, ignore_index=True)


def summarize(df: pd.DataFrame) -> str:
    lines = [
        "=== MEMPOOL STATISTICS BY NODE ===",
        "",
        "Metrics Explained:",
        "  Ready Txs     - Valid transactions that can execute immediately",
        "  Future Txs    - Valid transactions waiting for dependencies (e.g., nonce)",
        "  Admission Rate- Transactions submitted per second (TPS)",
        "  Validated     - Transactions checked for correctness",
        "  Revalidated   - Transactions re-validated after new blocks",
        "  Pruned        - Transactions included in finalized blocks",
        "  Reverified    - Transactions resubmitted after chain reorganization",
        ""
    ]
    for node, group in df.groupby("node"):
        g = group.dropna(subset=["ready", "future"])
        avg_ready = g["ready"].mean() if not g.empty else 0
        avg_future = g["future"].mean() if not g.empty else 0
        peak_ready = g["ready"].max() if not g.empty else 0
        peak_future = g["future"].max() if not g.empty else 0
        avg_adm = group["admission_tps"].mean() if "admission_tps" in group else 0
        
        # Additional metrics
        total_validated = group["validated"].sum() if "validated" in group else 0
        total_revalidated = group["revalidated"].sum() if "revalidated" in group else 0
        total_pruned = group["pruned"].sum() if "pruned" in group else 0
        total_reverified = group["reverified"].sum() if "reverified" in group else 0
        
        lines.append(f"Node: {node}")
        lines.append(f"  Average Ready Txs: {avg_ready:.2f}")
        lines.append(f"  Average Future Txs: {avg_future:.2f}")
        lines.append(f"  Peak Ready Txs: {int(peak_ready) if pd.notna(peak_ready) else 0}")
        lines.append(f"  Peak Future Txs: {int(peak_future) if pd.notna(peak_future) else 0}")
        lines.append(f"  Avg Admission Rate: {avg_adm:.2f} TPS")
        lines.append(f"  Total Validated: {int(total_validated)}")
        lines.append(f"  Total Revalidated: {int(total_revalidated)}")
        lines.append(f"  Total Pruned: {int(total_pruned)}")
        lines.append(f"  Total Reverified: {int(total_reverified)}")
        lines.append("")
    return "\n".join(lines)


def main():
    if len(sys.argv) < 3:
        print("Usage: python analyzer.py <mempool_report.txt> <analysis.txt> [window_ms]")
        sys.exit(1)
    report_path = sys.argv[1]
    out_path = sys.argv[2]
    window_ms = int(sys.argv[3]) if len(sys.argv) > 3 else 1000

    points = parse_report(report_path)
    df = to_dataframe(points)
    res = resample_metrics(df, window_ms)
    summary = summarize(res)

    with open(out_path, "w") as f:
        f.write("# Mempool Analysis\n\n")
        f.write(f"Window: {window_ms} ms\n\n")
        f.write(summary)

    print(f"Analysis saved to: {out_path}")


if __name__ == "__main__":
    main()
