import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = dirname(dirname(fileURLToPath(import.meta.url)));

function readHex(path) {
  const text = readFileSync(path, "utf8");
  const hex = text
    .split(/\r?\n/)
    .filter((line) => !line.trimStart().startsWith("#"))
    .join("")
    .replace(/\s/g, "");
  assert.match(hex, /^(?:[0-9a-fA-F]{2})*$/);
  return Buffer.from(hex, "hex");
}

function sha256(bytes) {
  return createHash("sha256").update(bytes).digest("hex");
}

function crc32c(bytes) {
  let crc = 0xffffffff;
  for (const byte of bytes) {
    crc ^= byte;
    for (let bit = 0; bit < 8; bit += 1) {
      crc = (crc >>> 1) ^ ((crc & 1) ? 0x82f63b78 : 0);
    }
  }
  return (~crc) >>> 0;
}

function number(value) {
  assert(value <= BigInt(Number.MAX_SAFE_INTEGER));
  assert(value >= BigInt(Number.MIN_SAFE_INTEGER));
  return Number(value);
}

function verifyEbex() {
  const directory = join(root, "crates", "ed-ebex", "fixtures");
  const bytes = readHex(join(directory, "ebex-v1-market.hex"));
  const expected = JSON.parse(readFileSync(join(directory, "ebex-v1-market.json"), "utf8"));
  assert.equal(bytes.length, expected.uncompressed_bytes);
  assert.equal(sha256(bytes), expected.uncompressed_sha256);
  assert.deepEqual([...bytes.subarray(0, 8)], [0x45, 0x42, 0x45, 0x58, 0, 0, 0, 0]);
  assert.equal(bytes.readUInt16LE(8), 1);
  assert.equal(bytes.readUInt16LE(10), 64);
  assert.equal(bytes.readUInt32LE(12), 1);

  const metadata = {
    sequence: number(bytes.readBigUInt64LE(16)),
    created_at: number(bytes.readBigInt64LE(24)),
    watermark: number(bytes.readBigInt64LE(32)),
    section_count: bytes.readUInt32LE(40),
  };
  assert.deepEqual(metadata, expected.metadata);
  assert.equal(bytes.readUInt16LE(44), 64);
  assert.equal(number(bytes.readBigUInt64LE(48)), 64);

  const at = 64;
  const section = {
    id: bytes.readUInt16LE(at),
    schema: bytes.readUInt16LE(at + 2),
    required: (bytes.readUInt32LE(at + 4) & 1) !== 0,
    record_count: number(bytes.readBigUInt64LE(at + 8)),
    record_size: bytes.readUInt32LE(at + 16),
  };
  const recordOffset = number(bytes.readBigUInt64LE(at + 24));
  const recordLength = number(bytes.readBigUInt64LE(at + 32));
  const auxiliaryOffset = number(bytes.readBigUInt64LE(at + 40));
  const auxiliaryLength = number(bytes.readBigUInt64LE(at + 48));
  const records = bytes.subarray(recordOffset, recordOffset + recordLength);
  const auxiliary = bytes.subarray(auxiliaryOffset, auxiliaryOffset + auxiliaryLength);
  assert.equal(crc32c(Buffer.concat([records, auxiliary])), bytes.readUInt32LE(at + 56));

  const market = [];
  for (let offset = 0; offset < records.length; offset += section.record_size) {
    market.push({
      station_id: number(records.readBigUInt64LE(offset)),
      commodity_id: records.readUInt16LE(offset + 8),
      buy_price: records.readUInt32LE(offset + 10),
      sell_price: records.readUInt32LE(offset + 14),
      demand: records.readUInt32LE(offset + 18),
      supply: records.readUInt32LE(offset + 22),
      observed_at: number(records.readBigInt64LE(offset + 26)),
    });
  }

  let offset = 0;
  const commodities = [];
  const commodityCount = auxiliary.readUInt16LE(offset);
  offset += 2;
  for (let index = 0; index < commodityCount; index += 1) {
    const id = auxiliary.readUInt16LE(offset);
    const lengths = [
      auxiliary.readUInt16LE(offset + 2),
      auxiliary.readUInt16LE(offset + 4),
      auxiliary.readUInt16LE(offset + 6),
    ];
    offset += 8;
    const fields = lengths.map((length) => {
      const value = new TextDecoder("utf-8", { fatal: true }).decode(auxiliary.subarray(offset, offset + length));
      offset += length;
      return value;
    });
    commodities.push({ id, symbol: fields[0], name: fields[1], category: fields[2] });
  }
  const stationCount = number(auxiliary.readBigUInt64LE(offset));
  offset += 8;
  const stations = [];
  for (let index = 0; index < stationCount; index += 1) {
    stations.push({
      station_id: number(auxiliary.readBigUInt64LE(offset)),
      observed_at: number(auxiliary.readBigInt64LE(offset + 8)),
    });
    offset += 16;
  }
  assert.equal(offset, auxiliary.length);
  assert.deepEqual({ ...section, records: market, auxiliary: { commodities, stations } }, expected.sections[0]);
  return bytes.length;
}

function ebexSections(bytes) {
  assert.deepEqual([...bytes.subarray(0, 8)], [0x45, 0x42, 0x45, 0x58, 0, 0, 0, 0]);
  const count = bytes.readUInt32LE(40);
  const sections = new Map();
  for (let index = 0; index < count; index += 1) {
    const at = 64 + index * 64;
    const id = bytes.readUInt16LE(at);
    const recordsAt = number(bytes.readBigUInt64LE(at + 24));
    const recordsLength = number(bytes.readBigUInt64LE(at + 32));
    const auxiliaryAt = number(bytes.readBigUInt64LE(at + 40));
    const auxiliaryLength = number(bytes.readBigUInt64LE(at + 48));
    const records = bytes.subarray(recordsAt, recordsAt + recordsLength);
    const auxiliary = bytes.subarray(auxiliaryAt, auxiliaryAt + auxiliaryLength);
    assert.equal(crc32c(Buffer.concat([records, auxiliary])), bytes.readUInt32LE(at + 56));
    sections.set(id, {
      schema: bytes.readUInt16LE(at + 2),
      required: (bytes.readUInt32LE(at + 4) & 1) !== 0,
      count: number(bytes.readBigUInt64LE(at + 8)),
      size: bytes.readUInt32LE(at + 16),
      records,
      auxiliary,
    });
  }
  return sections;
}

function stringTable(bytes) {
  const count = bytes.readUInt32LE(0);
  const values = new Map();
  let at = 4;
  for (let index = 0; index < count; index += 1) {
    const id = bytes.readUInt32LE(at);
    const length = bytes.readUInt32LE(at + 4);
    at += 8;
    values.set(id, new TextDecoder("utf-8", { fatal: true }).decode(bytes.subarray(at, at + length)));
    at += length;
  }
  assert.equal(at, bytes.length);
  return values;
}

function snapshotDirectory(bytes) {
  const count = number(bytes.readBigUInt64LE(0));
  const values = [];
  for (let index = 0; index < count; index += 1) {
    const at = 8 + index * 16;
    values.push({ station_id: number(bytes.readBigUInt64LE(at)), observed_at: number(bytes.readBigInt64LE(at + 8)) });
  }
  return values;
}

function verifyFullEbex() {
  const directory = join(root, "crates", "ed-ebex", "fixtures");
  const bytes = readHex(join(directory, "ebex-v1-full.hex"));
  const expected = JSON.parse(readFileSync(join(directory, "ebex-v1-full.json"), "utf8"));
  assert.equal(bytes.length, expected.uncompressed_bytes);
  assert.equal(sha256(bytes), expected.uncompressed_sha256);
  assert.deepEqual({
    sequence: number(bytes.readBigUInt64LE(16)),
    created_at: number(bytes.readBigInt64LE(24)),
    watermark: number(bytes.readBigInt64LE(32)),
    section_count: bytes.readUInt32LE(40),
  }, expected.metadata);
  const sections = ebexSections(bytes);
  assert.deepEqual([...sections.keys()], expected.section_ids);

  const systems = sections.get(1);
  const systemStrings = stringTable(systems.auxiliary);
  const system = systems.records;
  assert.deepEqual({
    address: number(system.readBigInt64LE(0)),
    position: [system.readDoubleLE(8), system.readDoubleLE(16), system.readDoubleLE(24)],
    population: number(system.readBigUInt64LE(32)),
    observed_at: number(system.readBigInt64LE(40)),
    name: systemStrings.get(system.readUInt32LE(48)),
    security: systemStrings.get(system.readUInt32LE(52)),
    allegiance: systemStrings.get(system.readUInt32LE(56)),
    controlling_power: systemStrings.get(system.readUInt32LE(60)),
    power_state: systemStrings.get(system.readUInt32LE(64)),
    powers: systemStrings.get(system.readUInt32LE(68)),
    flags: system.readUInt32LE(72),
  }, expected.system);

  const stations = sections.get(2);
  const stationStrings = stringTable(stations.auxiliary);
  const station = stations.records;
  assert.deepEqual({
    id: number(station.readBigUInt64LE(0)),
    system_address: number(station.readBigInt64LE(8)),
    name: stationStrings.get(station.readUInt32LE(16)),
    flags: station.readUInt32LE(20),
    market_observed_at: number(station.readBigInt64LE(24)),
    outfitting_observed_at: number(station.readBigInt64LE(32)),
    shipyard_observed_at: number(station.readBigInt64LE(40)),
  }, expected.station);

  const commodities = sections.get(4);
  const commodityStrings = stringTable(commodities.auxiliary);
  const commodity = commodities.records;
  assert.deepEqual({
    id: commodity.readUInt16LE(0),
    symbol: commodityStrings.get(commodity.readUInt32LE(4)),
    name: commodityStrings.get(commodity.readUInt32LE(8)),
    category: commodityStrings.get(commodity.readUInt32LE(12)),
  }, expected.commodity);

  const market = sections.get(5).records;
  assert.deepEqual({
    station_id: number(market.readBigUInt64LE(0)),
    commodity_id: market.readUInt16LE(8),
    buy_price: market.readUInt32LE(10),
    sell_price: market.readUInt32LE(14),
    demand: market.readUInt32LE(18),
    supply: market.readUInt32LE(22),
    observed_at: number(market.readBigInt64LE(26)),
  }, expected.market);

  for (const [id, key] of [[6, "module"], [8, "ship"]]) {
    const catalog = sections.get(id);
    const values = stringTable(catalog.auxiliary);
    assert.deepEqual({ id: catalog.records.readUInt32LE(0), symbol: values.get(catalog.records.readUInt32LE(4)) }, expected[key]);
  }
  for (const [id, key] of [[7, "outfitting"], [9, "shipyard"]]) {
    const availability = sections.get(id);
    const snapshots = snapshotDirectory(availability.auxiliary);
    assert.deepEqual({
      station_id: number(availability.records.readBigUInt64LE(0)),
      item_id: availability.records.readUInt32LE(8),
      observed_at: snapshots[0].observed_at,
    }, expected[key]);
  }

  // Stars v1 (section 16), 24 bytes/record, no auxiliary region: signed
  // address (negative = provisional), class code, scoopable flag,
  // reserved zeroes, observed_at. Strictly ascending by signed address.
  const stars = sections.get(16);
  assert.equal(stars.auxiliary.length, 0);
  assert.equal(stars.records.length % 24, 0);
  const starValues = [];
  for (let at = 0; at < stars.records.length; at += 24) {
    assert.ok(stars.records.subarray(at + 10, at + 16).every((b) => b === 0), "reserved star bytes are zero");
    starValues.push({
      address: number(stars.records.readBigInt64LE(at)),
      class: stars.records.readUInt8(at + 8),
      scoopable: stars.records.readUInt8(at + 9) === 1,
      observed_at: number(stars.records.readBigInt64LE(at + 16)),
    });
  }
  for (let i = 1; i < starValues.length; i += 1) {
    assert.ok(starValues[i - 1].address < starValues[i].address, "star records sorted by signed address");
  }
  assert.deepEqual(starValues, expected.stars);
  return bytes.length;
}

function verifyEdgx() {
  const directory = join(root, "crates", "ed-galaxy", "fixtures", "edgx-v2");
  const expected = JSON.parse(readFileSync(join(directory, "expected.json"), "utf8"));
  const files = Object.fromEntries(
    [["stars.bin", "stars.hex"], ["cells.bin", "cells.hex"], ["names.bin", "names.hex"], ["byname.bin", "byname.hex"]]
      .map(([name, hex]) => [name, readHex(join(directory, hex))]),
  );
  for (const [name, bytes] of Object.entries(files)) {
    assert.equal(bytes.length, expected.files[name].bytes);
    assert.equal(sha256(bytes), expected.files[name].sha256);
  }

  const stars = files["stars.bin"];
  assert.equal(stars.subarray(0, 4).toString("ascii"), "EDGX");
  assert.equal(stars.readUInt32LE(4), 2);
  const count = number(stars.readBigUInt64LE(8));
  const cellLy = stars.readFloatLE(16);
  assert.equal(cellLy, expected.cell_ly);
  const names = files["names.bin"];
  const records = [];
  for (let index = 0; index < count; index += 1) {
    const at = 32 + index * 29;
    const packed = stars[at + 12];
    const nameLength = stars.readUInt16LE(at + 14);
    const nameOffset = stars.readUIntLE(at + 24, 5);
    records.push({
      index,
      id64: number(stars.readBigUInt64LE(at + 16)),
      name: new TextDecoder("utf-8", { fatal: true }).decode(names.subarray(nameOffset, nameOffset + nameLength)),
      position: [stars.readFloatLE(at), stars.readFloatLE(at + 4), stars.readFloatLE(at + 8)],
      class_code: packed & 0x0f,
      flags: packed >>> 4,
    });
  }
  assert.deepEqual(records, expected.records);

  let covered = 0;
  let previousKey = -1n;
  const cells = files["cells.bin"];
  for (let at = 0; at < cells.length; at += 16) {
    const key = cells.readBigUInt64LE(at);
    const start = cells.readUInt32LE(at + 8);
    const members = cells.readUInt32LE(at + 12);
    assert(key > previousKey);
    assert.equal(start, covered);
    for (let index = start; index < start + members; index += 1) {
      const [x, y, z] = records[index].position.map((axis) => Math.floor(axis / cellLy));
      const pack = (axis) => BigInt(axis + 1_048_576);
      assert.equal((pack(x) << 42n) | (pack(y) << 21n) | pack(z), key);
    }
    covered += members;
    previousKey = key;
  }
  assert.equal(covered, count);

  const byname = files["byname.bin"];
  const indices = Array.from({ length: count }, (_, index) => byname.readUInt32LE(index * 4));
  assert.deepEqual(indices, expected.byname_indices);
  assert.deepEqual([...indices].sort((a, b) => a - b), Array.from({ length: count }, (_, index) => index));
  for (let index = 1; index < indices.length; index += 1) {
    const previous = records[indices[index - 1]].name.toLowerCase();
    const current = records[indices[index]].name.toLowerCase();
    assert(previous < current || (previous === current && indices[index - 1] < indices[index]));
  }
  return Object.values(files).reduce((total, bytes) => total + bytes.length, 0);
}

const ebexBytes = verifyEbex();
const fullEbexBytes = verifyFullEbex();
const edgxBytes = verifyEdgx();
console.log(`verified EBEX goldens (${ebexBytes} + ${fullEbexBytes} bytes) and EDGX golden (${edgxBytes} bytes)`);
