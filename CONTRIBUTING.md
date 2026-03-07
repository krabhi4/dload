# Contributing to DLoad

Thank you for your interest in contributing to DLoad! This document provides guidelines for contributing to this project.

## Table of Contents

- [Code of Conduct](#code-of-conduct)
- [Getting Started](#getting-started)
- [How to Contribute](#how-to-contribute)
- [Development Setup](#development-setup)
- [Pull Request Process](#pull-request-process)
- [Style Guidelines](#style-guidelines)
- [Reporting Bugs](#reporting-bugs)
- [Suggesting Features](#suggesting-features)

## Code of Conduct

By participating in this project, you agree to abide by our [Code of Conduct](CODE_OF_CONDUCT.md). Please read it before contributing.

## Getting Started

1. **Fork** the repository on GitHub
2. **Clone** your fork locally:
   ```bash
   git clone https://github.com/YOUR_USERNAME/dload.git
   cd dload
   ```
3. **Add the upstream remote**:
   ```bash
   git remote add upstream https://github.com/krabhi4/dload.git
   ```

## How to Contribute

- Fix bugs listed in the [issue tracker](https://github.com/krabhi4/dload/issues)
- Implement new features from the [roadmap](https://github.com/krabhi4/dload/issues?q=is%3Aissue+label%3Aenhancement)
- Improve documentation
- Write or improve tests
- Report bugs or suggest features

## Development Setup

### Prerequisites

- **Rust** (stable toolchain) — install via [rustup](https://rustup.rs/)
- **Docker** (optional, for containerized testing)

### Build & Run

```bash
# Build in debug mode
cargo build

# Run the application
cargo run

# Build optimized release binary
cargo build --release
./target/release/dload
```

### Running Tests

```bash
# Run all tests
cargo test

# Run with output shown
cargo test -- --nocapture

# Run a specific test
cargo test <test_name>
```

### Docker Build

```bash
docker build -t dload .
docker run -p 8080:8080 -v ./downloads:/data dload
```

## Pull Request Process

1. **Create a branch** from `main` with a descriptive name:
   ```bash
   git checkout -b feat/torrent-support
   # or
   git checkout -b fix/memory-leak-on-cancel
   ```

2. **Make your changes** following the [style guidelines](#style-guidelines) below.

3. **Commit your changes** with a clear, descriptive message following [Conventional Commits](https://www.conventionalcommits.org/):
   ```
   feat: add BitTorrent download support
   fix: resolve memory leak when cancelling downloads
   docs: update API endpoint documentation
   chore: upgrade reqwest to 0.13
   ```

4. **Keep your branch up to date**:
   ```bash
   git fetch upstream
   git rebase upstream/main
   ```

5. **Push** your branch and open a **Pull Request** against `main`.

6. Fill out the PR template — describe what changed and why.

7. A maintainer will review your PR. Please be responsive to feedback.

### PR Checklist

- [ ] Code compiles without warnings (`cargo build`)
- [ ] All tests pass (`cargo test`)
- [ ] `cargo clippy` produces no new warnings
- [ ] Code is formatted with `cargo fmt`
- [ ] Relevant documentation is updated
- [ ] CHANGELOG is updated (for significant changes)

## Style Guidelines

This project is written in Rust. Please follow the standard Rust conventions:

- **Formatting**: Use `cargo fmt` before committing — CI will enforce this.
- **Linting**: Run `cargo clippy` and address all warnings.
- **Error handling**: Prefer meaningful error types using `thiserror`. Avoid `unwrap()` in production paths.
- **Documentation**: Add doc comments (`///`) to all public functions, structs, and modules.
- **Testing**: Add unit tests for new logic. Integration tests for new API endpoints.

## Reporting Bugs

Before filing a bug, please check the [existing issues](https://github.com/krabhi4/dload/issues) to avoid duplicates.

When filing a bug report, please include:

- **DLoad version** (or commit hash)
- **OS and architecture** (e.g. macOS ARM64, Ubuntu x86_64)
- **Steps to reproduce** the issue
- **Expected behavior** vs **actual behavior**
- **Logs/error output** if available

Use the [Bug Report](https://github.com/krabhi4/dload/issues/new?template=bug_report.md) issue template.

## Suggesting Features

Feature requests are welcome! Please open an issue using the [Feature Request](https://github.com/krabhi4/dload/issues/new?template=feature_request.md) template and describe:

- The problem you're trying to solve
- The solution you'd like to see
- Any alternatives you've considered

## Questions

If you have questions, feel free to [open a discussion](https://github.com/krabhi4/dload/discussions) on GitHub.
