// Scan a relay-side packet capture for application payloads.
//
// Evidence for issue #19: a relay carries Rackio traffic without being able to
// read it. That claim is only worth something if it is checked against the
// bytes the relay actually saw, so this reads the capture taken on the relay's
// own interface and reports three things separately:
//
//   * what the relay can read — the endpoint identities it routes by, how many
//     packets it carried, when, and how many bytes. This is real metadata and
//     is reported as observed, not explained away;
//   * whether any needle the viewer read out of the session appears in the
//     bytes — the machine's display name, its node id, and metric values the
//     viewer displayed;
//   * whether any length-prefixed protobuf frame can be decoded out of the
//     stream. `rackio-protocol` frames a message as a big-endian u32 length
//     followed by the encoded protobuf, so this walks every offset, and at each
//     one tries to read that framing and parse the payload as protobuf wire
//     format. A parse that consumes exactly the framed length is counted as a
//     decodable frame.
//
// The lab's relay speaks plain HTTP, so this scan runs against a strictly
// weaker relay than production: nothing here is hidden by the relay's own TLS.
//
//   node scan-relay-capture.mjs <capture.pcap> <needles.json>
//
// `needles.json` is `[{ "label": ..., "kind": "utf8" | "hex" | "u64", "value": ... }]`.
// A `u64` needle is searched for as a protobuf varint, which is how a metric
// value would appear on the wire if the payload were readable.

import { readFileSync } from "node:fs";

const [, , capturePath, needlesPath] = process.argv;
if (!capturePath || !needlesPath) {
  process.stderr.write("usage: scan-relay-capture.mjs <capture.pcap> <needles.json>\n");
  process.exit(2);
}

/** Parse a libpcap file into per-packet timestamps and raw link-layer frames. */
function readPcap(buffer) {
  if (buffer.length < 24) {
    return { error: "the capture is shorter than a pcap file header", packets: [], linkType: null };
  }
  const magic = buffer.readUInt32LE(0);
  let little;
  if (magic === 0xa1b2c3d4 || magic === 0xa1b23c4d) {
    little = true;
  } else if (magic === 0xd4c3b2a1 || magic === 0x4d3cb2a1) {
    little = false;
  } else {
    return {
      error: `unrecognised pcap magic 0x${magic.toString(16)}`,
      packets: [],
      linkType: null,
    };
  }
  const u32 = (offset) => (little ? buffer.readUInt32LE(offset) : buffer.readUInt32BE(offset));
  const linkType = u32(20);
  const packets = [];
  let offset = 24;
  while (offset + 16 <= buffer.length) {
    const seconds = u32(offset);
    const fraction = u32(offset + 4);
    const captured = u32(offset + 8);
    const original = u32(offset + 12);
    offset += 16;
    if (captured > buffer.length - offset) break;
    packets.push({
      seconds,
      fraction,
      original,
      frame: buffer.subarray(offset, offset + captured),
    });
    offset += captured;
  }
  return { error: null, packets, linkType };
}

/**
 * Peel Ethernet/IPv4/TCP-or-UDP off a frame.
 *
 * Anything that is not IPv4 over Ethernet is reported rather than dropped: a
 * scan that silently skipped frames could report "no payload" about bytes it
 * never looked at.
 */
function dissect(frame, linkType) {
  if (linkType !== 1) return { kind: "unsupported-link-type" };
  if (frame.length < 14) return { kind: "short-frame" };
  const etherType = frame.readUInt16BE(12);
  if (etherType !== 0x0800) return { kind: `non-ipv4-ethertype-0x${etherType.toString(16)}` };
  const ip = frame.subarray(14);
  if (ip.length < 20) return { kind: "short-ip" };
  const headerLength = (ip[0] & 0x0f) * 4;
  const totalLength = ip.readUInt16BE(2);
  const protocol = ip[9];
  const source = Array.from(ip.subarray(12, 16)).join(".");
  const destination = Array.from(ip.subarray(16, 20)).join(".");
  const body = ip.subarray(headerLength, Math.min(totalLength, ip.length));
  if (protocol === 6) {
    if (body.length < 20) return { kind: "short-tcp", source, destination };
    const dataOffset = (body[12] >> 4) * 4;
    return {
      kind: "tcp",
      source,
      destination,
      sourcePort: body.readUInt16BE(0),
      destinationPort: body.readUInt16BE(2),
      payload: body.subarray(dataOffset),
    };
  }
  if (protocol === 17) {
    if (body.length < 8) return { kind: "short-udp", source, destination };
    return {
      kind: "udp",
      source,
      destination,
      sourcePort: body.readUInt16BE(0),
      destinationPort: body.readUInt16BE(2),
      payload: body.subarray(8),
    };
  }
  return { kind: `ip-protocol-${protocol}`, source, destination };
}

/** Encode a u64 the way protobuf would, so a metric value can be looked for. */
function varint(value) {
  const bytes = [];
  let remaining = BigInt(value);
  do {
    let byte = Number(remaining & 0x7fn);
    remaining >>= 7n;
    if (remaining > 0n) byte |= 0x80;
    bytes.push(byte);
  } while (remaining > 0n);
  return Buffer.from(bytes);
}

/**
 * Try to parse `bytes` as a complete protobuf message.
 *
 * Returns true only when every field is well formed and the parse ends exactly
 * at the end of the buffer. Protobuf has no self-identifying header, so a short
 * run of arbitrary bytes can parse by chance; requiring an exact end and at
 * least two fields keeps the false-positive rate low enough that a hit is worth
 * investigating rather than noise. A false positive here would make the report
 * *more* pessimistic about the relay, never less.
 */
function parsesAsProtobuf(bytes) {
  let offset = 0;
  let fields = 0;
  while (offset < bytes.length) {
    let key = 0n;
    let shift = 0n;
    let byte;
    do {
      if (offset >= bytes.length || shift > 63n) return false;
      byte = bytes[offset++];
      key |= BigInt(byte & 0x7f) << shift;
      shift += 7n;
    } while (byte & 0x80);
    const wireType = Number(key & 0x7n);
    const fieldNumber = key >> 3n;
    if (fieldNumber === 0n || fieldNumber > 536870911n) return false;
    if (wireType === 0) {
      do {
        if (offset >= bytes.length) return false;
        byte = bytes[offset++];
      } while (byte & 0x80);
    } else if (wireType === 1) {
      offset += 8;
    } else if (wireType === 2) {
      let length = 0n;
      shift = 0n;
      do {
        if (offset >= bytes.length || shift > 63n) return false;
        byte = bytes[offset++];
        length |= BigInt(byte & 0x7f) << shift;
        shift += 7n;
      } while (byte & 0x80);
      offset += Number(length);
    } else if (wireType === 5) {
      offset += 4;
    } else {
      return false;
    }
    if (offset > bytes.length) return false;
    fields += 1;
  }
  return offset === bytes.length && fields >= 2;
}

const MAX_FRAME_BYTES = 1_048_576;

/** Walk every offset looking for a `rackio-protocol` u32-length-prefixed frame. */
function decodableFrames(stream) {
  const hits = [];
  for (let offset = 0; offset + 4 <= stream.length; offset += 1) {
    const length = stream.readUInt32BE(offset);
    if (length < 8 || length > MAX_FRAME_BYTES) continue;
    if (offset + 4 + length > stream.length) continue;
    const body = stream.subarray(offset + 4, offset + 4 + length);
    if (parsesAsProtobuf(body)) {
      hits.push({ offset, length });
      if (hits.length >= 32) break;
    }
  }
  return hits;
}

const buffer = readFileSync(capturePath);
const needles = JSON.parse(readFileSync(needlesPath, "utf8"));
const { error, packets, linkType } = readPcap(buffer);

const conversations = new Map();
const payloads = [];
let payloadBytes = 0;
let firstSecond = null;
let lastSecond = null;
const unparsed = new Map();

for (const packet of packets) {
  const parsed = dissect(packet.frame, linkType);
  if (!parsed.payload) {
    unparsed.set(parsed.kind, (unparsed.get(parsed.kind) ?? 0) + 1);
    continue;
  }
  const key = `${parsed.source}:${parsed.sourcePort} -> ${parsed.destination}:${parsed.destinationPort} (${parsed.kind})`;
  const seen = conversations.get(key) ?? { packets: 0, payloadBytes: 0 };
  seen.packets += 1;
  seen.payloadBytes += parsed.payload.length;
  conversations.set(key, seen);
  if (parsed.payload.length > 0) {
    payloads.push(parsed.payload);
    payloadBytes += parsed.payload.length;
  }
  if (firstSecond === null) firstSecond = packet.seconds;
  lastSecond = packet.seconds;
}

const stream = Buffer.concat(payloads);
const findings = needles.map((needle) => {
  let pattern;
  if (needle.kind === "utf8") pattern = Buffer.from(String(needle.value), "utf8");
  else if (needle.kind === "hex") pattern = Buffer.from(String(needle.value), "hex");
  else if (needle.kind === "u64") pattern = varint(needle.value);
  else pattern = Buffer.alloc(0);
  return {
    label: needle.label,
    kind: needle.kind,
    expected_visible: needle.expected_visible ?? false,
    pattern_bytes: pattern.length,
    found: pattern.length > 0 && stream.includes(pattern),
  };
});

const frames = decodableFrames(stream);

process.stdout.write(
  `${JSON.stringify(
    {
      capture: capturePath,
      pcap_error: error,
      link_type: linkType,
      packets_in_capture: packets.length,
      frames_without_transport_payload: Object.fromEntries(unparsed),
      // What the relay can see, stated plainly.
      observable_metadata: {
        conversations: Object.fromEntries(conversations),
        payload_bytes_carried: payloadBytes,
        first_packet_epoch_seconds: firstSecond,
        last_packet_epoch_seconds: lastSecond,
        note: "endpoint identities, packet counts, byte volume and timing are visible to the relay and are recorded here rather than described as hidden",
      },
      needles: findings,
      decodable_protobuf_frames: frames,
      decodable_protobuf_frame_count: frames.length,
    },
    null,
    2,
  )}\n`,
);
