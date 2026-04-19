"""
Generate a 5,000-row DNA sequence CSV for FOINC task-distribution tests.

Rows have `id,sequence` with varying length (200-400 nt) and GC content
(30-70 %). Deterministic via fixed seed.
"""

import csv
import random
from pathlib import Path

N_SEQUENCES = 5000
LENGTH_MIN = 200
LENGTH_MAX = 400
GC_MIN = 0.30
GC_MAX = 0.70
SEED = 42
OUT = Path(__file__).with_name("sequences.csv")


def generate_sequence(length: int, gc_frac: float, rng: random.Random) -> str:
    at = (1.0 - gc_frac) / 2.0
    g = gc_frac / 2.0
    weights = [at, at, g, g]  # A, T, G, C
    bases = "ATGC"
    return "".join(rng.choices(bases, weights=weights, k=length))


def main() -> None:
    rng = random.Random(SEED)
    with OUT.open("w", newline="") as f:
        w = csv.writer(f)
        w.writerow(["id", "sequence"])
        for i in range(N_SEQUENCES):
            length = rng.randint(LENGTH_MIN, LENGTH_MAX)
            gc = rng.uniform(GC_MIN, GC_MAX)
            seq = generate_sequence(length, gc, rng)
            w.writerow([f"seq_{i:05d}", seq])
    print(f"wrote {N_SEQUENCES} sequences to {OUT}")


if __name__ == "__main__":
    main()
