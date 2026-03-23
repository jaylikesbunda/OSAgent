# Contributing to OSA

Thank you for your interest in contributing to OSA! This document provides guidelines and instructions for contributing.

## Code of Conduct

By participating in this project, you agree to maintain a respectful and inclusive environment for all contributors.

## How to Contribute

### Reporting Bugs

If you find a bug, please open an issue with:
1. A clear title and description
2. Steps to reproduce
3. Expected behavior
4. Actual behavior
5. Your environment (OS, version, etc.)

### Suggesting Features

Feature requests are welcome! Please open an issue with:
1. A clear title and description
2. Use case and motivation
3. Proposed solution (optional)

### Pull Requests

1. Fork the repository
2. Create a feature branch (`git checkout -b feature/amazing-feature`)
3. Make your changes
4. Run tests (`cargo test`)
5. Check formatting (`cargo fmt`)
6. Run clippy (`cargo clippy`)
7. Commit your changes (`git commit -m 'Add amazing feature'`)
8. Push to the branch (`git push origin feature/amazing-feature`)
9. Open a Pull Request

## Development Setup

### Prerequisites

- Rust 1.75 or later
- Git
- A code editor (VS Code, IntelliJ, etc.)

### Building

```bash
git clone https://github.com/YOUR_USERNAME/osagent.git
cd osagent
cargo build
```

### Testing

```bash
cargo test
```

### Formatting

```bash
cargo fmt
```

### Linting

```bash
cargo clippy
```

## Project Structure

```
osagent/
├── src/
│   ├── agent/        # Agent runtime, provider, sessions
│   ├── tools/        # Tool implementations
│   ├── web/          # HTTP server, API routes
│   ├── storage/      # SQLite storage layer
│   ├── discord/      # Optional Discord integration
│   ├── config.rs     # Configuration
│   └── main.rs       # Entry point
├── frontend/         # Web UI
├── docs/             # Setup and usage guides
├── examples/         # Example bundles and templates
└── .github/          # CI/CD workflows
```

## Coding Standards

### Rust

- Follow the [Rust API Guidelines](https://rust-lang.github.io/api-guidelines/)
- Use `clippy` for linting
- Format with `rustfmt`
- Document public APIs with doc comments

### Commits

- Use clear, descriptive commit messages
- Reference issues when applicable
- Keep commits focused and atomic

### Testing

- Write unit tests for new functionality
- Update integration tests as needed
- Ensure all tests pass before submitting PR

## Security

If you discover a security vulnerability, please email the maintainers directly instead of opening a public issue.

## License

By contributing to OSA, you agree that your contributions will be licensed under the GNU General Public License v3.

## Questions?

Feel free to open an issue for any questions or discussions.

Thank you for contributing to OSA! 🦞
