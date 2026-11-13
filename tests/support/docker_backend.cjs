// Local, dependency-free MCP fixture. Each container owns its own counter.
const { createInterface } = require('node:readline');
const { hostname } = require('node:os');
let count = 0;
createInterface({ input: process.stdin }).on('line', line => {
  const request = JSON.parse(line);
  if (request.id === undefined) return;
  let result;
  switch (request.method) {
    case 'initialize':
      result = { protocolVersion: request.params.protocolVersion,
        capabilities: { tools: {} }, serverInfo: { name: 'docker-fixture', version: '1.0.0' } };
      break;
    case 'tools/list':
      result = { tools: [{ name: 'state', description: 'Container-local counter',
        inputSchema: { type: 'object', properties: {} } }] };
      break;
    case 'tools/call':
      result = { content: [{ type: 'text', text: JSON.stringify({ count: ++count, host: hostname() }) }] };
      break;
    default:
      result = {};
  }
  process.stdout.write(`${JSON.stringify({ jsonrpc: '2.0', id: request.id, result })}\n`);
});
