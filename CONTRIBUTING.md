# Contributing to nextm

Thank you for your interest in nextm. The project adheres to two core principles for every change: **super simple and minimal** for the user, and **measured efficiency** for the developer. Every additional feature must address a genuine need; any additional cost (memory footprint, CPU cycles, timer wakeups, loaded DLLs) must be measured and justified.

## Reporting an Issue

Open an issue and attach the output of `nextm --diagnose` (Windows version, CPU, DPI, theme, settings mode). Clearly describe what you expected to happen and what actually occurred.

## Setting Up Your Environment

- Windows 11.
- Rust: the version is pinned in `rust-toolchain.toml`, and `rustup` installs it automatically.
- Visual Studio Build Tools with "Desktop development with C++" and the Windows SDK (including `rc.exe` for executable resources).

## Verification Commands

```powershell
cargo build --release -p nextm
cargo test --workspace
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo xtask check-imports target/release/nextm.exe
cargo xtask check-size target/release/nextm.exe --budget size-budget-x86_64-pc-windows-msvc.txt
cargo xtask bench target/release/nextm.exe --seconds 60 --warmup 45
```

Continuous Integration (CI) executes these exact checks for both x64 and ARM64.

## Code Guidelines

- All `unsafe` code must reside in `sys` modules, wrapped in RAII structures that automatically release resources, accompanied by a `// SAFETY:` explanation comment for every block.
- No `unwrap()`, `expect()`, or `panic!` calls in application runtime code (they are permitted in unit/integration tests).
- Zero heap allocations and no `format!` calls inside recurring sampling tick loops.
- Comments and docstrings should be clear and concise; identifiers must strictly be in English.
- **Do not add the `Win32_System_Registry` feature from windows-sys**: it declares registry APIs on advapi32, which causes the linker to bind statically to advapi32 instead of the lightweight API set (see `crates/nextm/src/sys/registry.rs`).
- Any optional DLL must be dynamically loaded with `LoadLibraryExW(..., LOAD_LIBRARY_SEARCH_SYSTEM32)` only when the feature that requires it is activated; `cargo xtask check-imports` will strictly fail if an unapproved static import appears.

## Reference Images (Golden Tests)

Tests in `crates/nextm-render/tests/golden.rs` compare rendered icons against bitmap references in `tests/golden/`. If an icon rendering adjustment is intentional, regenerate and review the diff before committing:

```powershell
$env:NEXTM_BLESS = "1"; cargo test -p nextm-render --test golden; Remove-Item Env:NEXTM_BLESS
```

## Binary Size Budget

`size-budget-<target>.txt` defines the reference binary size: CI fails if the compiled executable exceeds the reference by more than 5%. When binary size decreases, update the reference using `--update`.

## Branching Strategy

- `main`: production releases and milestone tags only (fully verified via tests, clippy, benchmarks, and review).
- `develop`: primary integration branch. Must always compile cleanly with all tests passing; pull requests should target `develop`.
- `feat/…`, `fix/…`: feature and bugfix branches. Branched from and merged back into `develop`.

## Commit Conventions

Keep commit messages concise and in the present tense, prefixed with standard conventional commit types (`feat:`, `fix:`, `perf:`, `docs:`, `test:`, `build:`, `ci:`). Every user-facing change must be documented in `CHANGELOG.md` under *Unreleased*.
