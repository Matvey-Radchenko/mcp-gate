# Setup wizard recording.
#
# A release binary on Linux exits before any prompt
# ("Automatic setup supports macOS and Windows only").
# This tape uses a local `cargo build --features test-backend --bin mcp-gate`
# and a throwaway home. It selects fixture connections, shows the preview,
# and cancels. Nothing is registered.
#
# Run from the repository root: vhs docs/assets/setup-demo.tape

Output "docs/assets/setup-demo.gif"

Set Shell "bash"
Set FontSize 16
Set Width 1040
Set Height 700
Set Padding 20
Set Framerate 8
Set TypingSpeed 25ms

Hide
Type `rm -rf /tmp/mcp-gate-demo && mkdir -p /tmp/mcp-gate-demo/.codex`
Enter
Type `printf '%s\n' '[mcp_servers.mail]' 'command = "/bin/true"' 'cwd = "/tmp/mcp-gate-demo"' '' '[mcp_servers.docs]' 'command = "/bin/true"' 'cwd = "/tmp/mcp-gate-demo"' > /tmp/mcp-gate-demo/.codex/config.toml`
Enter
Type `printf '%s\n' '{"mcpServers":{"browser":{"command":"/bin/true","args":[],"cwd":"/tmp/mcp-gate-demo"}}}' > /tmp/mcp-gate-demo/.claude.json`
Enter
Type `export HOME=/tmp/mcp-gate-demo MCP_GATE_TEST_HOME=/tmp/mcp-gate-demo MCP_GATE_TEST_ROOT=/tmp/mcp-gate-demo/state`
Enter
Type `export PATH="$PWD/target/debug:$PATH" PS1='$ ' TERM=xterm-256color`
Enter
Type `cd /tmp/mcp-gate-demo && clear`
Enter
Show

Sleep 400ms
Type "mcp-gate setup"
Enter
Sleep 1200ms
Enter
Sleep 2200ms
Type "mail"
Sleep 700ms
Space
Sleep 900ms
Ctrl+U
Sleep 400ms
Type "browser"
Sleep 700ms
Space
Sleep 1100ms
Enter
Sleep 4500ms
Type "can"
Sleep 600ms
Enter
Sleep 1500ms
