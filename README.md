# Pi Calculator (`pi_hesaplayici`)

`pi_hesaplayici` computes and mathematically certifies the fractional hexadecimal and decimal digits of $\pi$ using the **Bailey–Borwein–Plouffe (BBP)** formula with **directed-rounding MPFR interval arithmetic**, and independently verifies stored records using **Bellard's formula**.

Standard hardware floating-point (`f64`) arithmetic is **never used** for digit computation; no digit is ever committed unless its enclosing interval is proven to lie within a single integer cell.

---

## Table of Contents

1. [Mathematical Engine & Certification](#1-mathematical-engine--certification)
2. [GMP Base Conversion & $O(1)$ Record Validation](#2-gmp-base-conversion--o1-record-validation)
3. [Independent Verification (Bellard's Formula)](#3-independent-verification-bellards-formula)
4. [Record Schema, Atomic Persistence & Process Locking](#4-record-schema-atomic-persistence--process-locking)
5. [Operating Modes & Interfaces (GUI, TUI, Headless)](#5-operating-modes--interfaces-gui-tui-headless)
6. [Command-Line Options (CLI)](#6-command-line-options-cli)
7. [Module Architecture](#7-module-architecture)
8. [Building, Running & Testing](#8-building-running--testing)

---

## 1. Mathematical Engine & Certification

The BBP formula expresses the fractional part of $16^n \pi$ starting at hexadecimal index $n$ as a linear combination of four series:

$$\pi = \sum_{k=0}^{\infty} \frac{1}{16^k} \left( \frac{4}{8k+1} - \frac{2}{8k+4} - \frac{1}{8k+5} - \frac{1}{8k+6} \right)$$

* **Directed-Rounding Intervals (`Interval`):** Every intermediate addition, subtraction, and division is executed over a closed interval $[lo, hi]$ using `rug::Float` (MPFR), rounding the lower bound strictly downward (`Round::Down`) and the upper bound strictly upward (`Round::Up`).
* **Division-Free Montgomery Modular Exponentiation & Fused 4-Series Loop:** Finite terms ($0 \le k \le n$) are evaluated in a single fused loop over $j \in \{1, 4, 5, 6\}$ using 64-bit Montgomery reduction (`redc`) with Newton-Raphson modular inverses ($-m^{-1} \bmod 2^{64}$). For even denominators ($8k+4$ and $8k+6$), powers of two ($2^s = \gcd(d, 16^{n-k})$) are factored out so that all four sub-series execute division-free Montgomery binary exponentiation. At deeper indices ($\ge 4,000$ terms per thread), the range $0..=n$ is partitioned across CPU cores using strided slices (`StepBy`), giving every core an even balance of large and small exponents.
* **Analytic Tail Enclosure:** For $k > n$, the first $M = \text{precision}/4$ tail terms are summed explicitly as intervals, and the remaining infinite tail is bounded analytically by:
  $$\sum_{m=M+1}^{\infty} \frac{16^{-m}}{8(n+m)+j} \le \frac{1}{15 \cdot 16^M \cdot (8(n+M+1)+j)}$$
  and added with upward rounding to `hi`.
* **Adaptive Precision & Batch Extraction:** Evaluation starts at `128` bits of precision and extracts up to `20` hexadecimal digits (`MAX_BATCH_DIGITS`) per batch. Extraction stops at the first position where the scaled floor integers of `lo` and `hi` diverge. If not even a single digit agrees, precision doubles (`256, 512, …, 4096` bits); if `4096` bits still cannot certify a digit, an explicit error is returned.

---

## 2. GMP Base Conversion & $O(1)$ Record Validation

Certified hexadecimal digits (`hex_digits`) are converted into finalized decimal digits (`decimal_digits`) by [`DecimalConverter`](src/converter.rs):

* **Bit-Level GMP Arithmetic:** The converter operates entirely on `rug::Integer` (GMP). Because the denominator is $16^N = 2^{4N}$, no big-integer denominator or bitmask is ever allocated; the converter tracks `bits = 4N` and performs modulo and finality checks via in-place bit operations (`keep_bits_mut`, `significant_bits`).
* **Replay-Free Load Validation & Remainder Derivation (`from_digits`):** When loading a saved record, instead of replaying hundreds of thousands of hex digits one by one ($O(N^2)$), the loader derives the converter remainder $y = H \cdot 10^k - D \cdot 16^N$ directly from the hex integer $H$ ($N$ digits) and decimal integer $D$ ($k$ digits) in $O(1)$ GMP operations and verifies:
  $$0 \le y < 16^N \quad\text{and}\quad H \cdot 10^k = D \cdot 16^N + y$$
  together with the finality invariant that digit $k$ is final ($y + 10^k - 1 < 16^N$) and digit $k+1$ is not yet final. Because $y$ is uniquely determined by $(H, D)$, new snapshots omit `decimal_y_hex` on disk (reducing record file size by ~31%) while remaining fully backwards-compatible with existing v2 files that include `decimal_y_hex`.

---

## 3. Independent Verification (Bellard's Formula)

A `SHA-256` checksum only detects post-write file corruption. To verify the computation itself, **Bellard's series** ([`src/bellard.rs`](src/bellard.rs))—which uses completely different terms and denominators—serves as a second certified engine:

$$\pi = \frac{1}{2^6} \sum_{k=0}^{\infty} \frac{(-1)^k}{2^{10k}} \left( -\frac{2^5}{4k+1} - \frac{1}{4k+3} + \frac{2^8}{10k+1} - \frac{2^6}{10k+3} - \frac{2^2}{10k+5} - \frac{2^2}{10k+7} + \frac{1}{10k+9} \right)$$

* **Automatic Resume Spot-Check (`spot_check`):** Before resuming an existing record, the tail and 2 random windows are verified against Bellard's formula; on any mismatch the program aborts without modifying the record.
* **Sampled Verification (`--verify <file>`):** Checks the tail plus 32 random windows by default (configurable via `--samples <N>`).
* **Full Verification (`--verify <file> --samples all`):** Verifies every stored hex digit from index 0 to the end using Bellard's formula.
* **Legacy Record Migration (`--reverify <old> --output <new>`):** Re-certifies every digit of an uncertified v1 record from scratch and writes a validated v2 record.

---

## 4. Record Schema, Atomic Persistence & Process Locking

### Record File (`pi_record.json` — Schema v2)

| Field | Type | Description |
| :--- | :--- | :--- |
| `schema_version` | `u32` | Schema version (`2`). Unversioned v1 files are rejected unless migrated via `--reverify`. |
| `algorithm` | `String` | Algorithm identifier (`"bbp-mpfr-directed-v1"`). |
| `next_hex_index` | `u64` | Next hexadecimal index to compute (must equal `certified_hex_count`). |
| `certified_hex_count` | `u64` | Total number of certified hex digits (`hex_digits.len()`). |
| `total_elapsed_s` | `f64` | Cumulative compute time in seconds. |
| `min_elapsed_s` | `f64` | Fastest per-digit duration in seconds. |
| `max_elapsed_s` | `f64` | Slowest per-digit duration in seconds. |
| `hex_digits` | `String` | Uppercase fractional hexadecimal digits of $\pi$ (`243F6A88...`). |
| `decimal_digits` | `String` | Finalized fractional decimal digits (`14159265...`). |
| `digit_times` | `Vec<f64>` | Rolling history of the last 10 batch per-digit durations in seconds. |
| `decimal_y_hex` | `String` | *(Optional)* Converter remainder $y = H \cdot 10^k \bmod 16^N$ in uppercase hex; derived automatically from `(hex_digits, decimal_digits)` when omitted. |
| `hex_sha256` | `String` | Lowercase SHA-256 digest of `hex_digits`. |

### Atomic Writes & Advisory Process Lock (`.lock`)
* **Crash-Safe Atomic Save:** Snapshots are written to `.{filename}.tmp-{pid}` in the same directory, followed by `flush` $\to$ `sync_all` $\to$ atomic `rename` $\to$ parent directory `sync_all`.
* **Advisory Lock (`<output>.lock`):** Holds an OS-level exclusive flock (`try_lock_exclusive`) verified by Unix `dev`/`ino` identity checks. The lock file stores JSON metadata (`pid`, `started_unix_ms`, `state` as `running`/`paused`, `output_path`, `executable`, and any `recovered_stale_lock` from a previously crashed owner) and is unlinked on clean exit.
* **Signal Handling:** The first `SIGINT`, `SIGTERM`, or `SIGHUP` initiates a graceful stop (final atomic save + lock removal). A second signal terminates the process immediately.

---

## 5. Operating Modes & Interfaces (GUI, TUI, Headless)

### Desktop GUI (`--gui`, default)
A dark-themed `eframe`/`egui` analytical workstation:
* **KPI Cards & Milestone/Target Progress Bar:** Displays Certified Hex Digits, Finalized Decimal Digits, Throughput (`dig/s`) & Latency (`ms`), Total/Session Time & Extrema, plus a progress bar showing completion percentage and estimated time remaining (**ETA**) toward `--until` or the next automatic milestone.
* **10×10 Interactive Hex Matrix:** Visualizes the last 100 hex digits with high-contrast color coding, centered character labels, a bright border around the latest certified batch (20 digits), and per-cell hover tooltips (`D-<index> | Hex: <digit> (Dec: <val>)`).
* **Live Latency Area Sparkline:** Plots per-digit latency (`ms/digit`) over the last 100 digits with a translucent area fill, dashed average reference line, and interactive hover crosshair.
* **Digit Normality Histogram:** Plots full-record digit frequencies for `Hex 0-F` (`6.25%` expected) and `Dec 0-9` (`10.00%` expected) against a dashed uniform reference line with deviation ($\Delta$) tooltips.
* **Digit Explorer:** A 60 FPS virtual-scrolled (`show_rows`) indexed table (`+000,000 │ ... │ 000,050`, 50 digits per row in 5 groups of 10), with full-record **instant substring search (`🔍`)**, match navigation (`◀` / `▶` / `Enter`), inline match highlighting, and a 10k raw preview mode.
* **Fatal Error Preservation:** If the worker stops due to an error, the window stays open and displays a prominent red error banner (`⚠ WORKER STOPPED`) so the failure can be inspected.

| GUI Shortcut | Action |
| :--- | :--- |
| `Space` / `Ctrl+P` | Pause / Resume computation (when search input is not focused) |
| `Ctrl+S` | Trigger immediate atomic save |
| `Enter` | Jump to next match in the Digit Explorer search box |
| `Esc` / `Ctrl+Q` | Stop worker, perform final save, and exit |

### Terminal UI (`--tui`)
A flicker-free 80×24 `crossterm` interface using `EnterAlternateScreen` and hidden cursor:
* **Left Panel:** Stylized header, live `Hex:` / `Dec:` tail preview, formatted statistics, Unicode progress bar (`█`/`░`) + ETA, and Unicode latency sparkline (`▁▂▃▄▅▆▇█`).
* **Tabbed Right Panel:**
  * `[1:Compute Stream]`: Live stream of the last 21 certified digits.
  * `[2:Normality/Matrix]`: Live colored strip of the last 35 hex digits above a 16-row `0`–`F` frequency distribution table.

| TUI Shortcut | Action |
| :--- | :--- |
| `Space` / `P` / `Ctrl+P` | Pause / Resume computation |
| `S` / `Ctrl+S` | Trigger immediate atomic save |
| `Tab` / `1` / `2` | Switch right panel between `Compute Stream` and `Normality/Matrix` |
| `Q` / `Esc` / `Ctrl+C` | Stop worker, perform final save, and exit |

### Headless Mode (`--headless`)
Requires no terminal or display server; prints machine-readable progress lines every 10 seconds for unattended server runs:
```text
hex=160500 decimal=193261 last10_ms=1.531 session_s=10
```

---

## 6. Command-Line Options (CLI)

| Option | Description |
| :--- | :--- |
| `--gui` | Launch the desktop GUI (default when built with the `gui` feature). |
| `--tui` | Launch the terminal UI (default when built with `--no-default-features`). |
| `--headless` | Run without an interface, printing progress every 10 seconds. |
| `-o`, `--output <Path>` | Path to the computation record file (default: `pi_record.json`). |
| `--until <N>` | Stop automatically and save once the record reaches `N` hex digits. |
| `--at <Index>` | Compute and cross-verify hex digits starting at 0-based `<Index>` using both BBP and Bellard. |
| `--count <N>` | Number of hex digits to compute for `--at` (`1..=20`, default: `1`). |
| `--verify <Path>` | Verify a record file against Bellard's formula. |
| `--samples <N\|all>` | Verification scope for `--verify`: `N` random windows (default `32`) or `all`. |
| `--reverify <Path>` | Re-certify a legacy v1 record from scratch and write to `--output`. |
| `-h`, `--help` | Print command-line help and exit. |

---

## 7. Module Architecture

```text
src/
├── lib.rs          Crate root and architecture overview
├── main.rs         CLI entry point, signal wiring, and mode dispatcher
├── cli.rs          Command-line argument parser and conflict validation
├── math.rs         Certified MPFR BBP engine, interval arithmetic, and parallel summation
├── bellard.rs      Independent certified Bellard series engine for verification
├── converter.rs    GMP hex-to-decimal converter and O(1) invariant verification
├── record.rs       Schema v2 record model, SHA-256 hashing, and CoreState
├── storage.rs      Crash-safe atomic record writer and validated loader
├── lock.rs         Advisory (.lock) process lock and stale lock recovery
├── verify.rs       Resume spot-checks, Bellard verification, and v1 migration
├── worker.rs       Background compute thread, autosave loop, and error combination
├── app_state.rs    Concurrent state (SharedState) shared between worker and UIs
├── view.rs         Shared statistics, milestone/ETA, frequency, and search helpers
├── headless.rs     Unattended headless progress reporter
├── signals.rs      Unix signal handler (SIGINT, SIGTERM, SIGHUP)
├── tui/
│   ├── mod.rs      Terminal raw-mode/alternate-screen guard and event loop
│   └── render.rs   80×24 dual-panel tabbed terminal renderer
└── gui/
    ├── mod.rs      eframe/egui viewport setup and launcher
    ├── app.rs      GuiApp incremental state sync, shortcuts, and lifecycle
    ├── panels.rs   Top/bottom bars, KPI cards, 10×10 Hex matrix, and stream panel
    ├── charts.rs   Live latency area sparkline and digit normality histogram
    └── explorer.rs Virtual-scrolled indexed digit table and full-record search
```

---

## 8. Building, Running & Testing

This project links against native `GMP` and `MPFR` via `rug`. Always use `--release` (`opt-level = 3`, `lto = true`) for actual computation:

```bash
# Run with the desktop GUI
cargo run --release -- --gui

# Run with the terminal UI on a specific record file
cargo run --release -- --tui --output pi_record.json

# Run headless until 100,000 hex digits are certified
cargo run --release -- --headless --until 100000

# Query and cross-verify 8 hexadecimal digits starting at 0-based index 1,000,000
cargo run --release -- --at 1000000 --count 8

# Build without desktop GUI dependencies (TUI + Headless only)
cargo build --release --no-default-features

# Verify a record file with Bellard's formula (32 random windows or all digits)
cargo run --release -- --verify pi_record.json
cargo run --release -- --verify pi_record.json --samples all

# Formatting, lints, documentation, and test suite
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo doc --no-deps
cargo test
```
