# Setup wizard recording.
#
# Story: run setup, see the connections it found, select one,
# read the preview of what would change, then cancel.
#
# Recorded on Linux. Claude Desktop discovery runs only on macOS and Windows,
# so this recording shows Codex and Claude Code connections.
# This tape uses a local `cargo build --features test-backend --bin mcp-gate`
# and a throwaway home with three fixture connections. The fixture
# executables are stubs that exit immediately. Nothing is registered.
#
# Requires vhs (https://github.com/charmbracelet/vhs) with ttyd and ffmpeg.
# Run from the repository root: vhs docs/assets/setup-demo.tape

Output "docs/assets/setup-demo.gif"

Set Shell "bash"
Set FontSize 20
Set Width 1380
Set Height 680
Set Padding 28
Set Framerate 10
Set TypingSpeed 80ms
Set Theme "GitHub Dark"

Hide
Type `D=/tmp/mcp-gate-demo; rm -rf $D && mkdir -p $D/.codex $D/work $D/bin`
Enter
Type `for b in telegram-mcp github-mcp-server; do printf '#!/bin/sh\nexit 0\n' > $D/bin/$b; chmod +x $D/bin/$b; done`
Enter
Type `printf '%s\n' '[mcp_servers.telegram]' 'command = "telegram-mcp"' 'cwd = "/tmp/mcp-gate-demo/work"' '' '[mcp_servers.playwright]' 'command = "npx"' 'args = ["@playwright/mcp@latest"]' 'cwd = "/tmp/mcp-gate-demo/work"' > $D/.codex/config.toml`
Enter
Type `printf '%s\n' '{"mcpServers":{"github":{"command":"github-mcp-server","args":["stdio"],"cwd":"/tmp/mcp-gate-demo/work"}}}' > $D/.claude.json`
Enter
Type `export HOME=$D MCP_GATE_TEST_HOME=$D MCP_GATE_TEST_ROOT=$D/state`
Enter
Type `export PATH="$PWD/target/debug:$D/bin:$PATH" PS1='$ ' TERM=xterm-256color`
Enter
Type `cd $D && clear`
Enter
Sleep 500ms
Show

# 1. Run setup.
Sleep 800ms
Type "mcp-gate setup"
Sleep 600ms
Enter

# 2. Personal settings on this computer.
Sleep 2500ms
Enter

# 3. Found connections. Nothing is selected by default.
Sleep 3000ms
Down
Sleep 900ms
Space
Sleep 2000ms

# 4. Review the preview of what changes.
Enter
Sleep 6500ms

# 5. Cancel. Nothing was changed.
Down
Sleep 700ms
Down
Sleep 700ms
Down
Sleep 1200ms
Enter
Sleep 3500ms
