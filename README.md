# DLoad - Memory-Efficient Download Manager

A fast, lightweight download manager built in Rust that streams directly to disk without buffering in RAM. Solves aria2's high memory usage problem.

## Features

- **Low Memory Usage**: Streams chunks directly to disk (~20-50MB vs aria2's 100-500MB+)
- **HTTP/HTTPS Support**: Fast async downloading with progress tracking
- **Web UI**: Modern browser-based interface
- **REST API**: Programmatic access
- **SQLite Database**: Persistent download history
- **Docker Ready**: Multi-platform builds (amd64, arm64)

## Quick Start

### Docker Compose (Recommended)

```bash
# Clone and run
docker compose up -d

# Access at http://localhost:8080
```

### Docker Run

```bash
docker run -d \
  --name dload \
  -p 8080:8080 \
  -v ./downloads:/data \
  ghcr.io/yourusername/dload:latest
```

### Build from Source

```bash
cargo build --release
./target/release/dload
```

## Configuration

Default settings:
- **Port**: 8080
- **Download Directory**: `/data`
- **Max Concurrent**: 3 downloads
- **Username**: admin
- **Password**: admin

## API Endpoints

| Method | Endpoint | Description |
|--------|----------|-------------|
| POST | `/api/auth/login` | Login |
| GET | `/api/downloads` | List downloads |
| POST | `/api/downloads` | Add download |
| DELETE | `/api/downloads/:id` | Remove download |
| GET | `/api/settings` | Get settings |
| PUT | `/api/settings` | Update settings |

## Development

```bash
# Setup
cargo build

# Run
cargo run

# Docker build
docker build -t dload .
```

## Architecture

```
┌─────────────────────────────────────┐
│         Web UI (Vanilla JS)          │
└─────────────────┬───────────────────┘
                  │ HTTP
┌─────────────────▼───────────────────┐
│         REST API (Axum)              │
├─────────────────────────────────────┤
│      Download Manager               │
├──────────────┬──────────────────────┤
│  HTTP Worker │  (more protocols)   │
│  (reqwest)   │   coming soon       │
├──────────────┴──────────────────────┤
│    Direct to Disk (streaming)       │
└─────────────────────────────────────┘
```

## Memory Comparison

| Tool | Memory Usage |
|------|--------------|
| aria2 | 100-500MB+ |
| **dload** | **~20-50MB** |

## License

MIT
