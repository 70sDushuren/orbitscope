// WebSocket 位置流:订阅、自动重连、OSF1 二进制帧解码(格式见 docs/03-design.md 5.3 节)。

const MAGIC = [0x4f, 0x53, 0x46, 0x31]; // "OSF1"
const HEADER_LEN = 24;
const ENTRY_LEN = 16;

/**
 * 解码一帧。返回 { generation, timeMs, count, idx: Uint32Array, xyz: Float32Array(3*count) }。
 * 坐标为 ECEF km。
 */
export function decodeFrame(buf) {
  const bytes = new Uint8Array(buf);
  if (bytes.length < HEADER_LEN || MAGIC.some((b, i) => bytes[i] !== b)) {
    throw new Error('不是 OSF1 帧');
  }
  const dv = new DataView(buf);
  const version = dv.getUint32(4, true);
  if (version !== 1) throw new Error(`不支持的帧版本 ${version}`);
  const generation = dv.getUint32(8, true);
  const timeMs = dv.getFloat64(12, true) * 1000;
  const count = dv.getUint32(20, true);
  if (bytes.length !== HEADER_LEN + count * ENTRY_LEN) throw new Error('帧长度与条目数不符');
  const idx = new Uint32Array(count);
  const xyz = new Float32Array(count * 3);
  for (let k = 0, o = HEADER_LEN; k < count; k++, o += ENTRY_LEN) {
    idx[k] = dv.getUint32(o, true);
    xyz[3 * k] = dv.getFloat32(o + 4, true);
    xyz[3 * k + 1] = dv.getFloat32(o + 8, true);
    xyz[3 * k + 2] = dv.getFloat32(o + 12, true);
  }
  return { generation, timeMs, count, idx, xyz };
}

export class Stream {
  /**
   * @param {object} handlers { hello, catalog, frame, error, state(connected:boolean) }
   */
  constructor(handlers) {
    this.handlers = handlers;
    this.subscription = null;
    // 本连接的时间:null = 实时;数字 = "指定时刻"时钟相对本机时钟的偏移(毫秒)
    this.timeOffsetMs = null;
    this.retryMs = 1000;
    this.ws = null;
    this.closed = false;
  }

  url() {
    const proto = location.protocol === 'https:' ? 'wss:' : 'ws:';
    return `${proto}//${location.host}/api/v1/stream`;
  }

  connect() {
    const ws = new WebSocket(this.url());
    ws.binaryType = 'arraybuffer';
    this.ws = ws;
    ws.onopen = () => {
      this.retryMs = 1000;
      this.handlers.state?.(true);
      // 新连接:带上时间(服务端按连接保存;指定时刻的时钟一直在走,重连后从当前读数继续)
      if (this.subscription) this.send({ ...this.subscription, time: this.timeValue() });
    };
    ws.onmessage = (ev) => {
      if (typeof ev.data === 'string') {
        let msg;
        try {
          msg = JSON.parse(ev.data);
        } catch {
          return;
        }
        this.handlers[msg.type]?.(msg);
      } else {
        try {
          this.handlers.frame?.(decodeFrame(ev.data));
        } catch (e) {
          console.warn('丢弃无效帧:', e);
        }
      }
    };
    ws.onclose = () => {
      this.handlers.state?.(false);
      if (this.closed) return;
      setTimeout(() => this.connect(), this.retryMs);
      this.retryMs = Math.min(this.retryMs * 2, 10000);
    };
  }

  send(obj) {
    if (this.ws && this.ws.readyState === WebSocket.OPEN) this.ws.send(JSON.stringify(obj));
  }

  timeValue() {
    return this.timeOffsetMs === null ? 'live' : new Date(Date.now() + this.timeOffsetMs).toISOString();
  }

  /** 订阅(断线重连后自动重发)。不带时间:服务端保持本连接当前的时间模式。 */
  subscribe(filter, rateHz = 1) {
    this.subscription = { type: 'subscribe', filter, rate_hz: rateHz };
    this.send(this.subscription);
  }

  /**
   * 设置本连接的时间,只影响本页面:`null` 回到实时;否则从 `startMs`(UTC 毫秒)起回放,时钟继续走。
   */
  setTime(startMs) {
    this.timeOffsetMs = startMs === null ? null : startMs - Date.now();
    if (this.subscription) this.send({ ...this.subscription, time: this.timeValue() });
  }
}
