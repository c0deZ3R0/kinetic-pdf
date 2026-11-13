// Regression tests for the developer-only signature probe, not PDF validation.
// PATH: scripts/signatures/inspect.test.mjs
// CONSUMED BY: node --test; synthetic bytes only, no private sample fixtures.

import test from 'node:test';
import assert from 'node:assert/strict';
import { parseByteRange, signedBytes, unpadDer, inspectBytes } from './inspect.mjs';

const valid = '[0 10 20 10]';
test('two segments support a signed prefix and incremental unsigned tail', () => {
  assert.deepEqual(parseByteRange(valid, 30), [0, 10, 20, 10]);
  assert.deepEqual(parseByteRange(valid, 100), [0, 10, 20, 10]);
  assert.deepEqual(parseByteRange('[0\n10\t20 10]', 30), [0, 10, 20, 10]);
  const bytes = Buffer.from('0123456789abcdefghijABCDEFGHIJtail');
  assert.equal(signedBytes(bytes, [0, 10, 20, 10]).toString(), '0123456789ABCDEFGHIJ');
});

const malformed = [
  '[0\v10 20 10]', '[0\u00a010 20 10]',
  '[]', '[0 10 20]', '[0 10 20 10 30 5]', '[0 -1 20 10]', '[0 +10 20 10]',
  '[0 1.5 20 10]', '[0 1e1 20 10]', '[0 NaN 20 10]', '[0 Infinity 20 10]',
  '[0 0xA 20 10]', '[0 10/Name 20 10]', '[0 10 20 10]junk', '[0 10 20 10',
  '[1 10 20 10]', '[0 0 20 10]', '[0 10 20 0]', '[0 21 20 10]',
  '[0 20 20 10]', '[0 10 31 1]', '[0 10 20 11]',
  '[0 10 9007199254740992 1]', '[0 10 20 999999999999999999999999999999999]',
  `[0 ${'9'.repeat(1000)} 20 10]`,
];
for (const value of malformed) {
  test(`reject malformed range ${value.slice(0, 65)}`, () => {
    assert.throws(() => parseByteRange(value, 30));
  });
}

test('invalid file lengths and slicing inputs do not silently clamp', () => {
  for (const n of [-1, 1.5, Infinity, NaN, Number.MAX_SAFE_INTEGER + 1]) {
    assert.throws(() => parseByteRange(valid, n));
  }
  assert.throws(() => signedBytes(Buffer.alloc(30), [0, 10, 20, 11]));
});

test('DER length and PDF zero padding handling', () => {
  assert.deepEqual(unpadDer(Buffer.from([0x30, 1, 0, 0, 0])), Buffer.from([0x30, 1, 0]));
  const long = Buffer.concat([Buffer.from([0x30, 0x81, 0x80]), Buffer.alloc(128)]);
  assert.equal(unpadDer(long).length, 131);
  for (const bytes of [[], [0x31, 0], [0x30, 0x80], [0x30, 0x81],
    [0x30, 0x81, 1, 0], [0x30, 0x82, 0, 128], [0x30, 3, 0], [0x30, 0, 1]]) {
    assert.throws(() => unpadDer(Buffer.from(bytes)));
  }
});

// This is deliberately not a real PDF or CMS: it tests lexical gap validation only.
function fixture(gap = '<300100>') {
  const prefix = '%PDF-1.4\n9 0 obj <</Type/Sig/Contents ';
  const second = prefix.length + gap.length;
  const suffix = `/ByteRange [0 ${prefix.length} ${second} 0000000080]${' '.repeat(80)}>>endobj`;
  return Buffer.from(prefix + gap + suffix);
}

test('lexical extraction and exact gap boundaries', () => {
  const bytes = fixture();
  const result = inspectBytes(bytes);
  assert.equal(result.report.cmsBytes, 3);
  assert.equal(result.report.byteRange[1], bytes.indexOf(Buffer.from('<300100>')));
  assert.ok(result.report.unsignedTrailingBytes > 0);
  for (const gap of ['<30010>', '<30010001>', '<3001zz>', '<300100> ', ' <300100>', '(300100)']) {
    assert.throws(() => inspectBytes(fixture(gap)));
  }
});

test('ambiguous and non-array ByteRange fail before cryptography', () => {
  const bytes = fixture();
  assert.throws(() => inspectBytes(Buffer.concat([bytes, Buffer.from('/ByteRange [0 1 2 1]')])));
  assert.throws(() => inspectBytes(Buffer.from(bytes.toString().replace('/ByteRange [', '/ByteRange nope ['))));
  assert.throws(() => inspectBytes(Buffer.from(bytes.toString().replace('/Contents', '/NotContents'))));
  assert.throws(() => inspectBytes(Buffer.from('%PDF-1.7 unsigned')));
});
