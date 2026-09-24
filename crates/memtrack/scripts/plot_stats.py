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
MIB = 1024 * 1024
BIN_NS = 100_000_000
TIME_COLS = ("t", "t0", "t1", "stopped_at")


def load(path: Path) -> dict[str, pl.DataFrame]:
    rows = []
    for n, line in enumerate(path.read_text().splitlines(), 1):
        try:
            rows.append(json.loads(line))
        except json.JSONDecodeError:
            # A killed run can leave a cut-off last line.
            print(f"{path}:{n}: skipping unparsable line", file=sys.stderr)
    if not any(r["k"] == "ring" for r in rows):
        sys.exit(f"{path}: no ring records")
    t_min = min(r.get("t", r.get("t0", 0)) for r in rows)
    frames = {}
    for kind in ("ring_open", "ring", "backlog", "resolve", "encode"):
        kind_rows = [r for r in rows if r["k"] == kind]
        frames[kind] = pl.DataFrame(kind_rows).drop("k") if kind_rows else pl.DataFrame()
    pressure = [
        {"ring": r["ring"], "t": r["t"], "pid": pid, "stopped_at": at}
        for r in rows
        if r["k"] == "pressure"
        for pid, at in r["pids"]
    ]
    schema = {"ring": pl.Utf8, "t": pl.Int64, "pid": pl.Int64, "stopped_at": pl.Int64}
    frames["pressure"] = pl.DataFrame(pressure, schema=schema)
    for kind, df in frames.items():
        if kind != "ring_open" and len(df):
            frames[kind] = df.with_columns(pl.col(c) - t_min for c in TIME_COLS if c in df.columns)
    return frames


def episodes(pressure: pl.DataFrame) -> pl.DataFrame:
    """One row per released pid; episode bounds are shared by all pids released together."""
    return pressure.with_columns(start=pl.col("stopped_at").min().over("ring", "t"), end=pl.col("t"))


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


def busy_pct(intervals: pl.DataFrame) -> pl.DataFrame:
    """Share of each bin a thread spent inside short (t0, t1) work intervals."""
    return (
        intervals.select(t=pl.col("t1") // BIN_NS * BIN_NS, busy=pl.col("t1") - pl.col("t0"))
        .group_by("t").agg(pl.col("busy").sum()).sort("t")
        .select("t", pct=100 * pl.col("busy") / BIN_NS)
    )


def encoder_window_ns() -> pl.Expr:
    # Windows run back to back, so wait + encode + write is the wall time each one covers.
    return pl.col("wait_ns") + pl.col("encode_ns") + pl.col("write_ns")


def plot(f: dict[str, pl.DataFrame], eps: pl.DataFrame, out: Path) -> None:
    sizes = dict(zip(f["ring_open"]["ring"], f["ring_open"]["size"])) if len(f["ring_open"]) else {}
    rings, enc, backlog, resolve = f["ring"], f["encode"], f["backlog"], f["resolve"]
    n = 4 + bool(len(eps))
    fig, axes = plt.subplots(n, 1, sharex=True, figsize=(14, 3.2 * n))
    ax_fill, ax_tp, ax_busy, ax_backlog = axes[:4]

    # One color per ring across every panel.
    for i, name in enumerate(sorted(rings["ring"].unique())):
        r, color = rings.filter(pl.col("ring") == name).sort("t0"), f"C{i}"
        if size := sizes.get(name):
            xs = [v for a, b in zip(r["t0"], r["t1"]) for v in (a, b)]
            ys = [v for a, b in zip(r["prod0"] - r["cons0"], r["prod1"] - r["cons1"]) for v in (a, b)]
            ax_fill.plot([x / 1e9 for x in xs], [100 * y / size for y in ys], color=color, lw=0.8, label=name)
        w, d = write_rate(r), drain_rate(r)
        ax_tp.step(w["t1"] / 1e9, w["mbps"], where="pre", color=color, lw=0.8, label=f"{name} ring write")
        ax_tp.plot(d["t1"] / 1e9, d["mbps"], color=color, lw=0.8, ls=":", label=f"{name} drain (while busy)")
        b = busy_pct(r)
        ax_busy.step(b["t"] / 1e9, b["pct"], where="post", color=color, lw=0.8, label=f"{name} poller")
    ax_fill.axhline(75, ls="--", color="gray")
    for e in eps.unique(["ring", "t"]).iter_rows(named=True):
        for ax in axes:
            ax.axvspan(e["start"] / 1e9, e["end"] / 1e9, color="red", alpha=0.08, lw=0)
    ax_fill.set_ylabel("ring fill %")

    if len(enc):
        # A window can span seconds, so draw each one across the time it covers.
        e = enc.with_columns(window=encoder_window_ns())
        start, end = (e["t"] - e["window"]) / 1e9, e["t"] / 1e9
        for col, label, color in (("msgpack_bytes", "encoder in (msgpack)", "C6"), ("zstd_bytes", "encoder out (zstd, disk)", "C7")):
            ax_tp.hlines(e[col] / MB / (e["window"] / 1e9), start, end, color=color, lw=2, label=label)
        busy = 100 * (e["encode_ns"] + e["write_ns"]) / e["window"]
        ax_busy.hlines(busy, start, end, color="C6", lw=2, label="encoder (encode + write)")
    if len(resolve):
        b = busy_pct(resolve)
        ax_busy.step(b["t"] / 1e9, b["pct"], where="post", color="C5", lw=0.8, label="stack resolver")
    ax_tp.set_yscale("log")
    ax_tp.set_ylabel("MB/s (log)")
    ax_busy.set_ylabel("stage busy %")
    ax_busy.set_ylim(0, 105)

    if len(backlog):
        depth = (backlog["sent"] - backlog["received"]) / 1e6
        ax_backlog.plot(backlog["t"] / 1e9, depth, color="C2", lw=1, label="events in flight")
        ax_backlog.set_ylabel("M events in flight", color="C2")
        ax_rss = ax_backlog.twinx()
        ax_rss.plot(backlog["t"] / 1e9, backlog["rss"] / MIB, color="gray", lw=1, ls=":")
        ax_rss.set_ylabel("memtrack RSS (MiB)", color="gray")

    if len(eps):
        ax = axes[4]
        pids = sorted(eps["pid"].unique())
        for e in eps.iter_rows(named=True):
            ax.barh(pids.index(e["pid"]), (e["end"] - e["stopped_at"]) / 1e9, left=e["stopped_at"] / 1e9, color="C4")
        ax.set_yticks(range(len(pids)), [str(p) for p in pids])
        ax.set_ylabel("paused pid")
    for ax in axes:
        if ax.get_legend_handles_labels()[0]:
            ax.legend(loc="upper right", fontsize="small")
    axes[-1].set_xlabel("seconds")
    fig.tight_layout()
    fig.savefig(out, dpi=120)


def summary(f: dict[str, pl.DataFrame], eps: pl.DataFrame) -> None:
    sizes = dict(zip(f["ring_open"]["ring"], f["ring_open"]["size"])) if len(f["ring_open"]) else {}
    print(f"{'ring':<16} {'MB':>9} {'wr avg':>8} {'wr peak':>8} {'dr avg':>8} {'dr peak':>8}"
          f" {'fill%':>6} {'busy%':>6} {'eps':>4} {'paused ms':>10} {'max ms':>8}")
    for name, r in f["ring"].group_by("ring", maintain_order=True):
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

    if len(enc := f["encode"]):
        wall = enc.select(encoder_window_ns().sum()).item()
        msgpack, zstd = enc["msgpack_bytes"].sum(), enc["zstd_bytes"].sum()
        print(f"encoder: {len(enc)} windows, {enc['events'].sum()} events, {msgpack / MB:.1f} MB msgpack ->"
              f" {zstd / MB:.1f} MB zstd ({msgpack / max(zstd, 1):.1f}x), {msgpack / MB / (wall / 1e9):.1f} MB/s in,"
              f" busy {100 * (enc['encode_ns'].sum() + enc['write_ns'].sum()) / wall:.1f}%")
    if len(res := f["resolve"]):
        span = max(res["t1"].max() - res["t0"].min(), 1)
        print(f"resolver: {len(res)} batches, {res['n'].sum()} stacks, busy {100 * (res['t1'] - res['t0']).sum() / span:.1f}%")
    if len(bl := f["backlog"]):
        depth = bl["sent"] - bl["received"]
        print(f"backlog: peak {depth.max()} events in flight, peak RSS {bl['rss'].max() / MIB:.1f} MiB")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("stats", type=Path)
    parser.add_argument("-o", "--output", type=Path, default=Path("stats.png"))
    args = parser.parse_args()
    frames = load(args.stats)
    eps = episodes(frames["pressure"])
    plot(frames, eps, args.output)
    summary(frames, eps)
    print(f"wrote {args.output}")


if __name__ == "__main__":
    main()
