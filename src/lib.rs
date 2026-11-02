//! # Pi Calculator (`pi_hesaplayici`)
//!
//! Computes, mathematically certifies, verifies, and atomically persists the
//! fractional hexadecimal and decimal digits of $\pi$ using **directed-rounding
//! MPFR interval arithmetic** and **Bellard's independent formula**.
//!
//! ## Module Overview
//! - [`math`]: Adaptive-precision (`128..=4096` bits) BBP interval arithmetic engine and strided multi-core summation.
//! - [`bellard`]: Independent certified Bellard series engine used for verification.
//! - [`converter`]: GMP (`rug::Integer`) hexadecimal-to-decimal converter and $O(1)$ algebraic state verification ($H \cdot 10^k = D \cdot 16^N + y$).
//! - [`record`]: Schema v2 persisted record structure, SHA-256 integrity checks, and in-memory `CoreState`.
//! - [`storage`]: Crash-safe atomic file persistence and validated record loading.
//! - [`lock`]: Advisory process lock (`.lock`) with inode verification and stale lock recovery.
//! - [`verify`]: Resume spot-checks (`spot_check`), random/full Bellard verification (`--verify`), and v1 migration (`--reverify`).
//! - [`worker`]: Background compute worker thread, periodic autosave, and combined error reporting.
//! - [`app_state`]: Thread-safe shared state (`SharedState`) coordinating worker and UI threads.
//! - [`view`]: Shared presentation logic for statistics, milestone/ETA calculation, digit frequencies, and substring search.
//! - [`gui`]: Desktop `eframe`/`egui` analytical interface (enabled with the `gui` feature).
//! - [`tui`]: Tabbed 80×24 `crossterm` terminal interface.
//! - [`headless`]: Unattended machine-readable progress reporter.
//! - [`cli`]: Command-line argument parser.
//! - [`signals`]: Graceful Unix signal handling (`SIGINT`, `SIGTERM`, `SIGHUP`).

pub mod app_state;
pub mod bellard;
pub mod cli;
pub mod converter;
#[cfg(feature = "gui")]
pub mod gui;
pub mod headless;
pub mod lock;
pub mod math;
pub mod record;
#[cfg(unix)]
pub mod signals;
pub mod storage;
pub mod tui;
pub mod verify;
pub mod view;
pub mod worker;
