# Zed Discord RPC

Discord Rich Presence for the [Zed](https://zed.dev/) editor. Automatically shows what you're working on in Discord.

## Features

- Automatic workspace detection from Zed logs
- Language detection (Rust, TypeScript, Python, Go, Java, C/C++, Ruby, PHP, Swift, Dart, and more)
- Git branch detection
- Clean status with no emojis
- Auto-starts with your system
- Clears presence when Zed is closed
- Lightweight daemon (~2MB memory)

## Preview

```
┌─────────────────────────────────┐
│  my-project                     │
│  Rust | main                    │
│  Working on my-project          │
└─────────────────────────────────┘
```

## Installation

### Prerequisites

- [Rust](https://www.rust.org/tools/install) (via rustup)
- [Zed](https://zed.dev/) editor
- Discord running on your system

### Quick Install

```bash
git clone https://github.com/9sx77ssl/zed-discord-rpc.git
cd zed-discord-rpc
./install.sh
```

### Manual Install

1. **Build the extension:**
   ```bash
   cargo build --target wasm32-wasip2 --release
   ```

2. **Build the daemon:**
   ```bash
   cd daemon
   cargo build --release
   cp target/release/discord-rpc-daemon ~/.local/bin/zed-discord-rpc-daemon
   ```

3. **Install the extension in Zed:**
   - Open Zed
   - Run `zed: extensions`
   - Click "Install Dev Extension"
   - Select the `zed-discord-rpc` directory

4. **Start the daemon:**
   ```bash
   systemctl --user daemon-reload
   systemctl --user enable --now zed-discord-rpc
   ```

## Configuration

The Discord Application ID is configured in `daemon/src/main.rs`:

```rust
const DISCORD_APP_ID: &str = "YOUR_APP_ID";
```

To use your own Discord Application:

1. Go to [Discord Developer Portal](https://discord.com/developers/applications)
2. Create a new application
3. Copy the Application ID
4. Update the `DISCORD_APP_ID` constant
5. Rebuild the daemon

## Usage

Once installed, the daemon runs automatically in the background:

- **Check status:** `systemctl --user status zed-discord-rpc`
- **View logs:** `journalctl --user -u zed-discord-rpc -f`
- **Restart:** `systemctl --user restart zed-discord-rpc`
- **Stop:** `systemctl --user stop zed-discord-rpc`

## How It Works

1. **Extension** writes workspace info to `~/.local/state/zed-discord-rpc/workspace.json`
2. **Daemon** monitors Zed logs for workspace changes
3. **Daemon** updates Discord Rich Presence via IPC

## Supported Languages

| Language | Detection |
|----------|-----------|
| Rust | `Cargo.toml` |
| TypeScript | `package.json` |
| Python | `requirements.txt`, `pyproject.toml` |
| Go | `go.mod` |
| Java | `pom.xml`, `build.gradle` |
| C/C++ | `CMakeLists.txt`, `Makefile` |
| Ruby | `Gemfile` |
| PHP | `composer.json` |
| Swift | `Package.swift` |
| Dart | `pubspec.yaml` |

## Troubleshooting

### Discord not showing status

1. Make sure Discord is running
2. Check daemon logs: `journalctl --user -u zed-discord-rpc -f`
3. Restart daemon: `systemctl --user restart zed-discord-rpc`

### Extension not working

1. Check Zed logs: `zed: open log`
2. Make sure extension is installed: `zed: extensions`
3. Try running `/zed-rpc-update` manually

## License

MIT
