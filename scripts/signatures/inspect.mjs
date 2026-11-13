// Signature interoperability probe: read-only, sample-oriented PDF/CMS inspection.
// PATH: scripts/signatures/inspect.mjs
// CONSUMED BY: developers and inspect.test.mjs; requires local Node and OpenSSL.
// Not a PDF parser or security validator. Temporary CMS/content files are deleted.

import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import crypto from 'node:crypto';
import { spawnSync } from 'node:child_process';
import { pathToFileURL } from 'node:url';

const hash = bytes => crypto.createHash('sha256').update(bytes).digest('hex');
const fail = message => { throw new Error(message); };
const MAX_BYTES = 64 * 1024 * 1024;

// Deliberately supports exactly two nonempty segments, one Contents gap, offset 0.
// Parse with BigInt before conversion: never let Buffer.subarray clamp bad offsets.
export function parseByteRange(text, fileLength) {
  if (!Number.isSafeInteger(fileLength) || fileLength < 0) fail('Invalid file length');
  if (typeof text !== 'string' || text.length > 128
      || !/^\[[\x00\t\n\f\r ]*\d+(?:[\x00\t\n\f\r ]+\d+){3}[\x00\t\n\f\r ]*\]$/.test(text)) fail('Unsupported/malformed ByteRange');
  const values = text.match(/\d+/g).map(BigInt);
  if (values.some(n => n > BigInt(Number.MAX_SAFE_INTEGER))) fail('Unsafe ByteRange integer');
  const [start, length, second, tail] = values;
  if (start !== 0n || length === 0n || tail === 0n || second <= length
      || second + tail > BigInt(fileLength)) fail('Out-of-bounds/overlapping ByteRange');
  return values.map(Number);
}

export function signedBytes(bytes, range) {
  // Revalidate even when called independently of the lexical inspector.
  const [a, b, c, d] = parseByteRange(`[${range.join(' ')}]`, bytes.length);
  return Buffer.concat([bytes.subarray(a, a + b), bytes.subarray(c, c + d)]);
}

// DER SEQUENCE with definite, minimally encoded length; allow only zero PDF padding.
export function unpadDer(bytes) {
  if (bytes.length < 2 || bytes[0] !== 0x30) fail('CMS is not a DER SEQUENCE');
  let header = 2;
  let length = bytes[1];
  if (length & 0x80) {
    const count = length & 0x7f;
    if (!count || count > 4 || bytes.length < 2 + count || bytes[2] === 0) fail('Invalid DER length');
    length = 0;
    for (let i = 0; i < count; i++) length = length * 256 + bytes[2 + i];
    if (length < 128) fail('Nonminimal DER length');
    header += count;
  }
  const end = header + length;
  if (end > bytes.length || bytes.subarray(end).some(byte => byte !== 0)) fail('Truncated CMS/nonzero padding');
  return bytes.subarray(0, end);
}

export function inspectBytes(bytes) {
  const text = bytes.toString('latin1');
  // Lexical discovery only. Encoded names, streams, object streams, and duplicate
  // object resolution are NOT interpreted. Reject multiple raw ByteRange tokens.
  const keys = [...text.matchAll(/\/ByteRange\b/g)];
  if (keys.length !== 1) fail('Expected exactly one literal ByteRange (unsupported or ambiguous PDF)');
  const candidate = /^\/ByteRange[\x00\t\n\f\r ]*(\[[^\]]*\])/.exec(text.slice(keys[0].index));
  if (!candidate) fail('Malformed ByteRange array');
  const range = parseByteRange(candidate[1], bytes.length);
  const [, firstLength, secondStart, tailLength] = range;
  const gap = text.slice(firstLength, secondStart);
  if (!/^<[\da-fA-F\x00\t\n\f\r ]*>$/.test(gap)) fail('ByteRange gap is not exactly a hex Contents value');
  if (!/\/Contents[\x00\t\n\f\r ]*$/.test(text.slice(0, firstLength))) fail('Gap does not follow literal Contents');
  const hex = gap.slice(1, -1).replace(/[\x00\t\n\f\r ]/g, '');
  if (!hex.length || hex.length % 2) fail('Empty/odd Contents hex unsupported');
  const cms = unpadDer(Buffer.from(hex, 'hex'));
  const content = signedBytes(bytes, range);
  const names = key => [...text.matchAll(new RegExp(`/${key}\\s*/([^\\s<>/\\[\\]()]+)`, 'g'))].map(m => m[1]);
  return {
    cms, content,
    report: {
      bytes: bytes.length, sha256: hash(bytes), byteRange: range,
      signedRevisionEnd: secondStart + tailLength,
      unsignedTrailingBytes: bytes.length - secondStart - tailLength,
      signedContentSha256: hash(content), cmsBytes: cms.length, cmsSha256: hash(cms),
      lexicalOnly: {
        filters: names('Filter'), subFilters: names('SubFilter'),
        docMDPTokens: [...text.matchAll(/\/DocMDP\b/g)].length,
        transformMethods: names('TransformMethod'),
        documentTimestampTokens: [...text.matchAll(/\/DocTimeStamp\b/g)].length,
        prevOffsets: [...text.matchAll(/\/Prev\s+(\d+)/g)].map(m => Number(m[1])),
        startxrefOffsets: [...text.matchAll(/startxref\s+(\d+)/g)].map(m => Number(m[1])),
      },
    },
  };
}

function openssl(args) {
  const result = spawnSync(process.env.OPENSSL || 'openssl', args, {
    encoding: 'utf8', timeout: 30000, maxBuffer: 4 * 1024 * 1024, windowsHide: true,
  });
  if (result.error || result.signal || result.status === null) fail('OpenSSL unavailable, timed out, or exceeded output limit');
  return result;
}

export function checkCms(cms, content) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'kinetic-signature-'));
  try {
    const cmsPath = path.join(dir, 'signature.der');
    const dataPath = path.join(dir, 'signed-content.bin');
    const outputPath = path.join(dir, 'verified.bin');
    fs.writeFileSync(cmsPath, cms, { mode: 0o600 });
    fs.writeFileSync(dataPath, content, { mode: 0o600 });
    const verify = () => openssl(['cms', '-verify', '-binary', '-inform', 'DER', '-in', cmsPath,
      '-content', dataPath, '-noverify', '-out', outputPath]).status === 0;
    if (!verify()) fail('CMS integrity verification failed (trust was not checked)');
    if (!fs.readFileSync(outputPath).equals(content)) fail('OpenSSL output differs from signed content');
    const changedContent = Buffer.from(content);
    changedContent[0] ^= 1;
    fs.writeFileSync(dataPath, changedContent);
    if (verify()) fail('Negative control failed: changed signed content accepted');
    fs.writeFileSync(dataPath, content);
    const changedCms = Buffer.from(cms);
    changedCms[changedCms.length - 1] ^= 1;
    fs.writeFileSync(cmsPath, changedCms);
    if (verify()) fail('Negative control failed: changed CMS accepted');
    fs.writeFileSync(cmsPath, cms);
    // Keep the raw output internal: certificate subjects/serials are unnecessary.
    const printed = openssl(['cms', '-cmsout', '-print', '-inform', 'DER', '-in', cmsPath]);
    if (printed.status !== 0) fail('Cannot inspect CMS');
    const p = printed.stdout;
    return {
      integrity: 'PASS (-binary -noverify; NOT certificate trust)',
      changedSignedContentRejected: true, changedCmsRejected: true,
      algorithms: [...new Set([...p.matchAll(/algorithm: ([^\r\n]+)/g)].map(m => m[1]))],
      signedAttributesAbsent: /\bsignedAttrs:\s*<ABSENT>/.test(p),
      signerDigestAlgorithms: [...p.matchAll(/digestAlgorithm:\s*\n\s+algorithm: ([^\r\n]+)/g)].map(m => m[1]),
      signerSignatureAlgorithms: [...p.matchAll(/signatureAlgorithm:\s*\n\s+algorithm: ([^\r\n]+)/g)].map(m => m[1]),
      signingTimes: [...p.matchAll(/object: signingTime[^\n]*\n\s+set:\s*\n\s+(?:UTCTIME|GENERALIZEDTIME):([^\r\n]+)/g)].map(m => m[1]),
      signatureTimestampTokenPresent: /1\.2\.840\.113549\.1\.9\.16\.2\.14\)/.test(p),
      tstInfoPresent: /1\.2\.840\.113549\.1\.9\.16\.1\.4\)/.test(p),
      counterSignaturePresent: /1\.2\.840\.113549\.1\.9\.6\)/.test(p),
      unsignedAttributesAbsent: /unsignedAttrs:\s*<ABSENT>/.test(p),
    };
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
}

export function inspectFile(file) {
  if (fs.statSync(file).size > MAX_BYTES) fail('Probe limited to 64 MiB inputs');
  const bytes = fs.readFileSync(file);
  if (bytes.length > MAX_BYTES) fail('Probe limited to 64 MiB inputs');
  const inspected = inspectBytes(bytes);
  return { bytes, ...inspected, report: { file: path.basename(file), ...inspected.report,
    cms: checkCms(inspected.cms, inspected.content) } };
}

if (process.argv[1] && import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href) {
  try {
    const files = process.argv.slice(2);
    if (files.length < 1 || files.length > 2) fail('Usage: node scripts/signatures/inspect.mjs ORIGINAL.pdf [MARKED.pdf]');
    const results = files.map(inspectFile);
    const report = { openssl: openssl(['version']).stdout.trim(), files: results.map(r => r.report) };
    if (results.length === 2) {
      const [a, b] = results;
      report.comparison = {
        originalIsExactPrefix: b.bytes.length >= a.bytes.length && b.bytes.subarray(0, a.bytes.length).equals(a.bytes),
        signedContentIdentical: a.content.equals(b.content), cmsIdentical: a.cms.equals(b.cms),
        appendedBytes: b.bytes.length - a.bytes.length,
      };
    }
    console.log(JSON.stringify(report, null, 2));
  } catch (error) {
    console.error(`Inspection rejected: ${error.message}`);
    process.exitCode = 1;
  }
}
