# /// script
# requires-python = ">=3.11"
# dependencies = ["polars", "matplotlib"]
# ///
"""Plot memtrack pipeline stats written via CODSPEED_MEMTRACK_STATS."""

import argparse
import json
import sys
from pathlib import Path

import matplotlib

matplotlib.use("Agg")
import matplotlib.pyplot as plt
import polars as pl

MB = 1e6
BIN_NS = 100_000_000


def load(path: Path) -> tuple[pl.DataFrame, pl.DataFrame, dict[str, int]]:
    rows = [json.loads(line) for line in path.read_text().splitlines() if line.strip()]
    if not rows:
        sys.exit(f"{path}: no records")
    sizes = {r["ring"]: r["size"] for r in rows if r["k"] == "ring_open"}
    ring_rows = [r for r in rows if r["k"] == "ring"]
    if not ring_rows:
        sys.exit(f"{path}: no ring records")
    pressure_rows = [
        {"ring": r["ring"], "t": r["t"], "pid": pid, "stopped_at": at}
        for r in rows
        if r["k"] == "pressure"
        for pid, at in r["pids"]
    ]
    t_min = min(r.get("t", r.get("t0", 0)) for r in rows)
    rings = pl.DataFrame(ring_rows).drop("k")
    rings = rings.with_columns(pl.col("t0", "t1") - t_min)
    schema = {"ring": pl.Utf8, "t": pl.Int64, "pid": pl.Int64, "stopped_at": pl.Int64}
    pressure = pl.DataFrame(pressure_rows, schema=schema)
    pressure = pressure.with_columns(pl.col("t", "stopped_at") - t_min)
    return rings, pressure, sizes


def episodes(pressure: pl.DataFrame) -> pl.DataFrame:
    """One row per released pid; episode bounds are shared by all pids released together."""
    return pressure.with_columns(
        start=pl.col("stopped_at").min().over("ring", "t"), end=pl.col("t")
    )


def write_rate(r: pl.DataFrame) -> pl.DataFrame:
    # Positions are cumulative bytes; spread each delta over the real gap since
    # the previous sample, since samples are sparse when the ring is idle.
    r = r.sort("t1")
    dt = pl.col("t1").diff()
    return r.select("t1", mbps=pl.col("prod1").diff() / MB / (dt / 1e9)).filter(dt > 0)


def drain_rate(r: pl.DataFrame) -> pl.DataFrame:
    # Aggregated per bin: ticks of a few us give meaningless per-tick ratios.
    return (
        r.select(t1=pl.col("t1") // BIN_NS * BIN_NS, bytes=pl.col("cons1") - pl.col("cons0"), busy=pl.col("t1") - pl.col("t0"))
        .group_by("t1").agg(pl.col("bytes", "busy").sum()).sort("t1")
        .filter(pl.col("busy") > 0)
        .select("t1", mbps=pl.col("bytes") / MB / (pl.col("busy") / 1e9))
    )


def plot(rings: pl.DataFrame, eps: pl.DataFrame, sizes: dict[str, int], out: Path) -> None:
    n = 3 if len(eps) else 2
    fig, axes = plt.subplots(n, 1, sharex=True, figsize=(14, 3.5 * n), squeeze=False)
    ax_fill, ax_tp = axes[0][0], axes[1][0]
    for name, r in rings.sort("t0").group_by("ring", maintain_order=True):
        name, size = name[0], sizes.get(name[0])
        if size:
            xs = [v for a, b in zip(r["t0"], r["t1"]) for v in (a, b)]
            ys = [v for a, b in zip(r["prod0"] - r["cons0"], r["prod1"] - r["cons1"]) for v in (a, b)]
            ax_fill.plot([x / 1e9 for x in xs], [100 * y / size for y in ys], lw=0.8, label=name)
        w, d = write_rate(r), drain_rate(r)
        ax_tp.step(w["t1"] / 1e9, w["mbps"], where="pre", lw=0.8, label=f"{name} write")
        ax_tp.plot(d["t1"] / 1e9, d["mbps"], lw=0.8, ls=":", label=f"{name} drain")
    ax_fill.axhline(75, ls="--", color="gray")
    for e in eps.unique(["ring", "t"]).iter_rows(named=True):
        ax_fill.axvspan(e["start"] / 1e9, e["end"] / 1e9, color="red", alpha=0.15)
    ax_fill.set_ylabel("fill %")
    ax_tp.set_ylabel("MB/s")
    if len(eps):
        ax = axes[2][0]
        pids = sorted(eps["pid"].unique())
        colors = {ring: f"C{i}" for i, ring in enumerate(sorted(eps["ring"].unique()))}
        for e in eps.iter_rows(named=True):
            ax.barh(pids.index(e["pid"]), (e["end"] - e["stopped_at"]) / 1e9,
                    left=e["stopped_at"] / 1e9, color=colors[e["ring"]])
        ax.set_yticks(range(len(pids)), [str(p) for p in pids])
        ax.set_ylabel("paused pid")
    for row in axes:
        if row[0].get_legend_handles_labels()[0]:
            row[0].legend(loc="upper right", fontsize="small")
    axes[-1][0].set_xlabel("seconds")
    fig.tight_layout()
    fig.savefig(out, dpi=120)


def summary(rings: pl.DataFrame, eps: pl.DataFrame, sizes: dict[str, int]) -> None:
    print(f"{'ring':<16} {'MB':>9} {'wr avg':>8} {'wr peak':>8} {'dr avg':>8} {'dr peak':>8}"
          f" {'fill%':>6} {'busy%':>6} {'eps':>4} {'paused ms':>10} {'max ms':>8}")
    for name, r in rings.group_by("ring", maintain_order=True):
        name = name[0]
        span = max(r["t1"].max() - r["t0"].min(), 1)
        written = r["prod1"].max() - r["prod0"].min()
        w, d = write_rate(r), drain_rate(r)
        size = sizes.get(name)
        fill = 100 * max((r["prod0"] - r["cons0"]).max(), (r["prod1"] - r["cons1"]).max()) / size if size else float("nan")
        busy = 100 * (r["t1"] - r["t0"]).sum() / span
        e = eps.filter(pl.col("ring") == name)
        paused = (e["end"] - e["stopped_at"]) / 1e6
        print(f"{name:<16} {written / MB:>9.1f} {written / MB / (span / 1e9):>8.1f} {w['mbps'].max() or 0:>8.1f}"
              f" {d['mbps'].mean() or 0:>8.1f} {d['mbps'].max() or 0:>8.1f} {fill:>6.1f} {busy:>6.1f}"
              f" {e.unique('t').height:>4} {paused.sum():>10.1f} {paused.max() or 0:>8.1f}")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("stats", type=Path)
    parser.add_argument("-o", "--output", type=Path, default=Path("stats.png"))
    args = parser.parse_args()
    rings, pressure, sizes = load(args.stats)
    eps = episodes(pressure)
    plot(rings, eps, sizes, args.output)
    summary(rings, eps, sizes)
    print(f"wrote {args.output}")


if __name__ == "__main__":
    main()
