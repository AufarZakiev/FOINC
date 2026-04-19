"""
DNA sequence analyzer — one task per CSV row.

Contract (per FOINC task-distribution): `sys.argv = ["<user-script>", <id>, <sequence>]`.
Emits a single CSV line on stdout with derived features.

Uses numpy (array ops, FFT) and scipy.stats (chi-square) to keep the
worker busy for a realistic amount of time per row (typically
50-400 ms depending on sequence length).
"""

import sys
import numpy as np
from scipy import fft
from scipy.stats import chisquare

# --------------------------------------------------------------------------- #
# Feature helpers
# --------------------------------------------------------------------------- #

_NT_SIGNAL = {"A": 1.0, "T": -1.0, "G": 2.0, "C": -2.0}
_COMP = {"A": "T", "T": "A", "G": "C", "C": "G", "N": "N"}
_STOPS = {"TAA", "TAG", "TGA"}
_START = "ATG"


def gc_content(seq: str) -> float:
    arr = np.frombuffer(seq.encode("ascii"), dtype=np.uint8)
    mask = (arr == ord("G")) | (arr == ord("C"))
    return float(mask.mean())


def shannon_entropy_kmer(seq: str, k: int = 3) -> float:
    """Entropy of the k-mer distribution in bits."""
    n = len(seq)
    if n < k:
        return 0.0
    kmers = np.array([seq[i : i + k] for i in range(n - k + 1)])
    _, counts = np.unique(kmers, return_counts=True)
    probs = counts / counts.sum()
    return float(-np.sum(probs * np.log2(probs)))


def melting_temp(seq: str) -> float:
    """Tm (Celsius): Wallace rule for short seqs, GC-corrected formula otherwise."""
    n = len(seq)
    if n < 14:
        at = seq.count("A") + seq.count("T")
        gc = seq.count("G") + seq.count("C")
        return 2.0 * at + 4.0 * gc
    return 64.9 + 0.41 * (gc_content(seq) * 100.0) - (500.0 / n)


def reverse_complement(seq: str) -> str:
    return "".join(_COMP.get(b, "N") for b in reversed(seq))


def longest_orf_length(seq: str) -> int:
    """Longest open reading frame length across all 6 frames, in nucleotides."""
    best = 0
    for strand in (seq, reverse_complement(seq)):
        for frame in range(3):
            current_start = None
            i = frame
            while i + 3 <= len(strand):
                codon = strand[i : i + 3]
                if current_start is None:
                    if codon == _START:
                        current_start = i
                else:
                    if codon in _STOPS:
                        best = max(best, i - current_start)
                        current_start = None
                i += 3
            if current_start is not None:
                best = max(best, i - current_start)
    return best


def fft_dominant_freq(seq: str) -> float:
    """Magnitude of the dominant non-DC frequency in the one-hot-like signal."""
    n = len(seq)
    if n < 2:
        return 0.0
    signal = np.fromiter((_NT_SIGNAL.get(b, 0.0) for b in seq), dtype=float, count=n)
    spectrum = np.abs(fft.fft(signal))
    return float(np.max(spectrum[1 : n // 2])) if n >= 4 else 0.0


def codon_bias_chi2(seq: str) -> float:
    """Chi-square statistic: observed codon usage vs uniform expectation."""
    n = len(seq)
    if n < 60:
        return 0.0
    codons = np.array([seq[i : i + 3] for i in range(0, n - 2, 3)])
    unique, counts = np.unique(codons, return_counts=True)
    if len(unique) < 2:
        return 0.0
    expected = np.full(len(unique), counts.sum() / len(unique))
    res = chisquare(counts.astype(float), expected)
    return float(res.statistic)


# --------------------------------------------------------------------------- #
# Main
# --------------------------------------------------------------------------- #


def main() -> None:
    if len(sys.argv) < 3:
        raise SystemExit("usage: <seq_id> <sequence>")
    seq_id = sys.argv[1]
    sequence = sys.argv[2].upper()

    # Guard against stray whitespace or non-DNA characters by keeping only ACGTN
    sequence = "".join(b for b in sequence if b in "ACGTN")

    features = (
        seq_id,
        len(sequence),
        round(gc_content(sequence), 4),
        round(shannon_entropy_kmer(sequence, k=3), 4),
        round(melting_temp(sequence), 2),
        longest_orf_length(sequence),
        round(fft_dominant_freq(sequence), 2),
        round(codon_bias_chi2(sequence), 2),
    )

    # Single CSV row on stdout
    print(",".join(str(f) for f in features))


if __name__ == "__main__":
    main()
