# Setup wizard recording, happy path.
#
# Story: run setup, see the connections it found, select Context7, read the
# preview, apply it, then check `mcp-gate status`.
#
# Recorded on Linux. Claude Desktop discovery runs only on macOS and Windows,
# so this recording shows Codex and Claude Code connections.
# This tape uses a local `cargo build --features test-backend --bin mcp-gate`
# and a throwaway home with three connections. Context7 is real: the
# docs/assets/demo/context7-mcp wrapper runs `npx -y @upstash/context7-mcp`
# (the committed GIF was recorded with 4.7.3; with 4.8.0 and later, setup adds
# a note that the skills extension is not forwarded).
# The other two servers are stubs and are not selected. The recording machine
# has no `systemd --user`, so docs/assets/demo/systemctl stands in for it and
# starts the gateway in the background. Everything stays under /tmp.
#
# Requires vhs (https://github.com/charmbracelet/vhs) with ttyd and ffmpeg,
# plus Node.js with npx. Run from the repository root:
#   vhs docs/assets/setup-demo.tape
# Afterwards stop the demo gateway: pkill -f /tmp/mcp-gate-demo/state

Output "docs/assets/setup-demo.gif"

Set Shell "bash"
Set FontSize 20
Set Width 1380
Set Height 760
Set Padding 28
Set Framerate 10
Set TypingSpeed 80ms
Set Theme "GitHub Dark"

Hide
Type `pkill -f /tmp/mcp-gate-demo/state; D=/tmp/mcp-gate-demo; rm -rf $D && mkdir -p $D/.codex $D/work $D/bin`
Enter
Type `printf '#!/bin/sh\nexit 0\n' > $D/bin/github-mcp-server; chmod +x $D/bin/github-mcp-server`
Enter
Type `printf '%s\n' '[mcp_servers.context7]' 'command = "context7-mcp"' 'cwd = "/tmp/mcp-gate-demo/work"' '' '[mcp_servers.playwright]' 'command = "npx"' 'args = ["@playwright/mcp@latest"]' 'cwd = "/tmp/mcp-gate-demo/work"' > $D/.codex/config.toml`
Enter
Type `printf '%s\n' '{"mcpServers":{"github":{"command":"github-mcp-server","args":["stdio"],"cwd":"/tmp/mcp-gate-demo/work"}}}' > $D/.claude.json`
Enter
Type `export HOME=$D MCP_GATE_TEST_HOME=$D MCP_GATE_TEST_ROOT=$D/state XDG_CONFIG_HOME=$D/.config XDG_DATA_HOME=$D/.local/share`
Enter
Type `export PATH="$PWD/docs/assets/demo:$PWD/target/debug:$D/bin:$PATH" PS1='$ ' TERM=xterm-256color`
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

# 3. Found connections. Nothing is selected by default. Select Context7.
Sleep 2500ms
Space
Sleep 1500ms

# 4. Review the preview of what changes.
Enter
Sleep 5500ms

# 5. Apply. Setup checks the server, starts the gateway, and edits the Codex config.
Enter
Sleep 17000ms

# 6. Check the result. The backend has not started yet.
Type "mcp-gate status"
Sleep 600ms
Enter
Sleep 6000ms
