// Node.js 22+: node examples/oled-upload.mjs DISCOVERY IMAGE DEVICE_ID '[TARGET_JSON]'
// This writes the selected image resource on the specified keyboard.
import { readFile, stat } from 'node:fs/promises';
import { createHash, randomUUID } from 'node:crypto';
import { setTimeout as sleep } from 'node:timers/promises';

const [discoveryPath, imagePath, deviceId, targetJson] = process.argv.slice(2);
if (!discoveryPath || !imagePath || !deviceId) {
  throw new Error('Usage: node examples/oled-upload.mjs DISCOVERY IMAGE DEVICE_ID \'{"kind":"modeDefault","mode":0}\'');
}
const target = targetJson ? JSON.parse(targetJson) : { kind: 'modeDefault', mode: 0 };
const discovery = JSON.parse(await readFile(discoveryPath, 'utf8'));
const endpoint = new URL(discovery.endpoint);
if (endpoint.protocol !== 'ws:' || endpoint.hostname !== '127.0.0.1') throw new Error('Expected local Runtime');
if ((await stat(imagePath)).size > 2 * 1024 * 1024) throw new Error('Image exceeds 2 MiB');
const source = await readFile(imagePath);
const socket = new WebSocket(discovery.endpoint);
const pending = new Map();
let serial = 0;
let chunkReply;
let operationId;
socket.addEventListener('message', ({ data }) => {
  const message = JSON.parse(data);
  if (message.id != null) {
    const call = pending.get(message.id);
    if (!call) return;
    pending.delete(message.id);
    clearTimeout(call.timer);
    if (message.error) call.reject(new Error(message.error.data?.code ?? message.error.message));
    else call.resolve(message.result);
  } else if (message.method === 'resource.progress' || message.method === 'resource.error') {
    if (!chunkReply) return;
    const reply = chunkReply;
    chunkReply = undefined;
    clearTimeout(reply.timer);
    if (message.method === 'resource.error') reply.reject(new Error(message.params.code));
    else reply.resolve(message.params);
  } else if (message.method === 'runtime.event' && message.params.type === 'operation.changed') {
    const operation = message.params.data;
    if (operation.operationId === operationId) {
      process.stderr.write(`\r${operation.status}: ${Math.floor(operation.progress * 100)}%   `);
    }
  }
});
socket.addEventListener('close', () => {
  for (const call of pending.values()) { clearTimeout(call.timer); call.reject(new Error('Runtime disconnected')); }
  pending.clear();
  if (chunkReply) { clearTimeout(chunkReply.timer); chunkReply.reject(new Error('Runtime disconnected')); chunkReply = undefined; }
});
function rpc(method, params) {
  return new Promise((resolve, reject) => {
    const id = String(++serial);
    const timer = setTimeout(() => { pending.delete(id); reject(new Error(`${method} timed out`)); }, 180_000);
    pending.set(id, { resolve, reject, timer });
    socket.send(JSON.stringify({ jsonrpc: '2.0', id, method, params }));
  });
}
try {
  await new Promise((resolve, reject) => {
    socket.addEventListener('open', resolve, { once: true });
    socket.addEventListener('error', reject, { once: true });
  });
  await rpc('runtime.hello', { protocol: { major: 1, minor: 0 },
    client: { name: 'oled-upload-example', version: '1', kind: 'cli' }, token: discovery.token });
  const state = await rpc('oled.get', { deviceId });
  console.log(`Device profile: ${state.profile}; ${state.width}×${state.height}; ${state.intervalMs}ms/frame`);
  await rpc('runtime.subscribe', { topics: ['state'], cursor: null });
  const upload = await rpc('resource.begin', { byteLength: source.length,
    sha256: createHash('sha256').update(source).digest('hex') });
  const uuid = Buffer.from(upload.uploadId.replaceAll('-', ''), 'hex');
  for (let offset = 0; offset < source.length; offset += upload.chunkBytes) {
    const payload = source.subarray(offset, offset + upload.chunkBytes);
    const header = Buffer.alloc(20);
    uuid.copy(header);
    header.writeUInt32LE(offset, 16);
    await new Promise((resolve, reject) => {
      const timer = setTimeout(() => { chunkReply = undefined; reject(new Error('Resource chunk timed out')); }, 30_000);
      chunkReply = { resolve, reject, timer };
      socket.send(Buffer.concat([header, payload]));
    });
  }
  const resource = await rpc('resource.finish', { uploadId: upload.uploadId });
  const baseline = await rpc('configuration.get', { deviceId });
  operationId = randomUUID();
  console.log(`Operation ID: ${operationId}`);
  await rpc('oled.upload', { operationId, deviceId, baseToken: baseline.baseToken,
    resourceId: resource.resourceId, target });
  for (;;) {
    const operation = await rpc('operation.get', { operationId });
    if (operation.status === 'completed') { console.log('\nUpload committed and metadata verified.'); break; }
    if (operation.status === 'failed' || operation.status === 'paused') {
      throw new Error(`${operation.status}: ${operation.errorCode}; operationId=${operationId}`);
    }
    await sleep(300);
  }
} finally {
  socket.close();
}
