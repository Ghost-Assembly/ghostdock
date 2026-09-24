// MCP, spoken to by the official MCP client library over real HTTP: the
// same code MCP clients are built on. Lists tools, deploys a stack and
// waits for it, reads its logs, runs a command in it, and finds a revoked
// token refused.
//
// Needs a live server with the Livebox fixture deployed. See run.sh.
import { Client } from '@modelcontextprotocol/sdk/client/index.js';
import { StreamableHTTPClientTransport } from '@modelcontextprotocol/sdk/client/streamableHttp.js';

const BASE = process.env.GHOSTDOCK_URL ?? 'http://127.0.0.1:18081';
const COOKIE = process.env.GHOSTDOCK_COOKIE;
const problems = [];
const check = (ok, what) => { if (!ok) problems.push(what); };

async function api(method, path, body) {
  const res = await fetch(`${BASE}/api/v1${path}`, {
    method,
    headers: { cookie: COOKIE, 'content-type': 'application/json' },
    body: body ? JSON.stringify(body) : undefined,
  });
  return res.status === 204 ? null : res.json();
}

async function connect(secret) {
  const client = new Client({ name: 'ghostdock-e2e', version: '1.0.0' });
  const transport = new StreamableHTTPClientTransport(new URL(`${BASE}/mcp`), {
    requestInit: { headers: { authorization: `Bearer ${secret}` } },
  });
  await client.connect(transport);
  return client;
}

const text = result => result.content.map(c => c.text).join('\n');

const { secret } = await api('POST', '/tokens', {
  name: 'assistant',
  permissions: ['host.view', 'logs.view', 'stacks.deploy', 'shell.open'],
  expires_in_days: 1,
});
const client = await connect(secret);
console.log('server        :', `${client.getServerVersion().name} ${client.getServerVersion().version}`);

const { tools } = await client.listTools();
const names = tools.map(t => t.name);
console.log('tools         :', `${names.length}: ${names.join(', ')}`);
check(names.includes('deploy_stack') && names.includes('run_command'), 'granted tools are missing');
check(!names.includes('take_down_stack') && !names.includes('cleanup'), 'tools outside the token are listed');

const stacks = await client.callTool({ name: 'list_stacks', arguments: {} });
check(!stacks.isError && text(stacks).includes('livebox'), `list_stacks: ${text(stacks).slice(0, 200)}`);
console.log('list_stacks   :', stacks.isError ? 'ERROR' : 'livebox listed');

const t0 = Date.now();
const deployed = await client.callTool({ name: 'deploy_stack', arguments: { stack: 'livebox' } });
const outcome = deployed.structuredContent ?? JSON.parse(text(deployed));
console.log('deploy_stack  :', `${outcome.status} after ${Math.round((Date.now() - t0) / 1000)}s, exit ${outcome.exit_code}`);
check(!deployed.isError && outcome.status === 'succeeded', `deploy did not succeed: ${text(deployed).slice(0, 300)}`);

const logs = await client.callTool({ name: 'container_logs', arguments: { container: 'livebox-box-1', lines: 20 } });
console.log('container_logs:', logs.isError ? `ERROR ${text(logs)}` : text(logs).split('\n')[0]);
check(!logs.isError, 'container_logs failed');

const ran = await client.callTool({
  name: 'run_command',
  arguments: { container: 'livebox-box-1', command: 'echo from-mcp; cat /etc/alpine-release; exit 4' },
});
const run = ran.structuredContent ?? JSON.parse(text(ran));
console.log('run_command   :', `exit ${run.exit_code}, output ${JSON.stringify(run.output)}`);
check(run.exit_code === 4 && run.output.includes('from-mcp'), 'run_command did not report output and exit code');

// Two 5 s ticks after the deploy, so the container has a rate to report.
await new Promise(r => setTimeout(r, 12000));
const figures = await client.callTool({ name: 'get_metrics', arguments: { subject: 'livebox-box-1', range: '1h' } });
const fig = figures.structuredContent ?? JSON.parse(text(figures));
console.log('get_metrics   :', `${fig.points} points, cpu average ${fig.cpu?.average}, memory peak ${fig.memory?.peak}`);
check(typeof fig.cpu?.average === 'number', 'get_metrics gave no CPU figure');

const missing = await client.callTool({ name: 'get_stack', arguments: { stack: 'no-such-stack' } });
console.log('a bad name    :', missing.isError ? `reported: ${text(missing)}` : 'NOT an error');
check(missing.isError, 'a missing stack was not reported as an error');

// Recorded under the token's name.
const audit = await api('GET', '/audit');
const ranEntry = audit.find(e => e.action === 'run command');
console.log('audited as    :', ranEntry?.username, `(${ranEntry?.detail})`);
check(ranEntry?.username === 'admin (token assistant)', 'the command was not audited under the token');

// Revoked, the next call fails.
const tokens = await api('GET', '/tokens');
await api('DELETE', `/tokens/${tokens.find(t => t.name === 'assistant').id}`);
const after = await client.callTool({ name: 'list_stacks', arguments: {} }).then(() => 'still works', e => `refused (${e.message.slice(0, 60)})`);
console.log('after revoke  :', after);
check(after.startsWith('refused'), 'a revoked token still worked');

await client.close().catch(() => {});
console.log(problems.length ? `\nPROBLEMS:\n- ${problems.join('\n- ')}` : '\nThe official MCP client works with GhostDock.');
process.exit(problems.length ? 1 : 0);
