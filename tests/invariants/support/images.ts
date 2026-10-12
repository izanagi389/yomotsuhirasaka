// メディアの約束（INV-MED-*）で使うファイルを作る。外部のライブラリを使わず、ここで組み立てる。

import { crc32, deflateSync } from "node:zlib";

// ---------------------------------------------------------------------------
// PNG
// ---------------------------------------------------------------------------

function pngChunk(type: string, data: Buffer): Buffer {
  const len = Buffer.alloc(4);
  len.writeUInt32BE(data.length);
  const typeAndData = Buffer.concat([Buffer.from(type, "ascii"), data]);
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(crc32(typeAndData) >>> 0);
  return Buffer.concat([len, typeAndData, crc]);
}

const PNG_SIGNATURE = Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]);

function ihdr(width: number, height: number, bitDepth: number, colorType: number): Buffer {
  const d = Buffer.alloc(13);
  d.writeUInt32BE(width, 0);
  d.writeUInt32BE(height, 4);
  d.writeUInt8(bitDepth, 8);
  d.writeUInt8(colorType, 9);
  return pngChunk("IHDR", d);
}

/** RGBの単色のPNG。exif を渡すと eXIf チャンクとして埋め込む。 */
export function pngImage(width: number, height: number, options: { exif?: Buffer } = {}): Buffer {
  const row = Buffer.alloc(1 + width * 3);
  for (let x = 0; x < width; x++) {
    row[1 + x * 3] = (x * 7) & 0xff;
    row[2 + x * 3] = 120;
    row[3 + x * 3] = 200;
  }
  const raw = Buffer.concat(Array.from({ length: height }, () => row));
  const chunks = [PNG_SIGNATURE, ihdr(width, height, 8, 2)];
  if (options.exif) chunks.push(pngChunk("eXIf", options.exif));
  chunks.push(pngChunk("IDAT", deflateSync(raw)), pngChunk("IEND", Buffer.alloc(0)));
  return Buffer.concat(chunks);
}

/** 圧縮の効かない（ほぼランダムな）PNG。容量の上限の確認に使う。ファイルの大きさはおよそ width×height×3 バイト。 */
export function pngNoise(width: number, height: number): Buffer {
  const rows: Buffer[] = [];
  let x = 0x12345678;
  for (let y = 0; y < height; y++) {
    const row = Buffer.alloc(1 + width * 3);
    for (let i = 1; i < row.length; i++) {
      x ^= x << 13;
      x ^= x >>> 17;
      x ^= x << 5;
      row[i] = x & 0xff;
    }
    rows.push(row);
  }
  return Buffer.concat([PNG_SIGNATURE, ihdr(width, height, 8, 2), pngChunk("IDAT", deflateSync(Buffer.concat(rows))), pngChunk("IEND", Buffer.alloc(0))]);
}

/**
 * 画素数が非常に大きいのに、ファイルは小さいPNG（解凍爆弾）。1ビットの白黒で、中身はすべて0。
 * 既定の 20000×20000（4億画素）で、ファイルは数十KB程度。
 */
export function pngPixelBomb(width = 20_000, height = 20_000): Buffer {
  const rowBytes = 1 + Math.ceil(width / 8);
  const raw = Buffer.alloc(rowBytes * height); // フィルター0・画素0
  return Buffer.concat([PNG_SIGNATURE, ihdr(width, height, 1, 0), pngChunk("IDAT", deflateSync(raw, { level: 9 })), pngChunk("IEND", Buffer.alloc(0))]);
}

/** 大きさだけを宣言し、画素のデータはほとんど持たないPNG（ヘッダーだけで上限を判定できるかの確認用）。 */
export function pngHeaderOnlyHuge(width = 100_000, height = 100_000): Buffer {
  return Buffer.concat([PNG_SIGNATURE, ihdr(width, height, 8, 2), pngChunk("IDAT", deflateSync(Buffer.alloc(1024))), pngChunk("IEND", Buffer.alloc(0))]);
}

// ---------------------------------------------------------------------------
// EXIF（位置情報）
// ---------------------------------------------------------------------------

/** 位置情報の中に入れる目印。保存後のファイルに残っていてはいけない。 */
export const GPS_MARKER = "INV-GPS-MARKER-5c1d";

/**
 * 位置情報（GPS IFD）を含むTIFF形式のEXIF（"Exif\0\0" の後に続く部分）。
 * GPS IFD には、緯度と、目印を入れた GPSAreaInformation を入れる。
 */
export function exifWithGps(): Buffer {
  // リトルエンディアン。IFD0: 1件（GPSInfo）。GPS IFD: 3件（LatitudeRef, Latitude, AreaInformation）。
  const area = Buffer.concat([Buffer.from("ASCII\0\0\0", "latin1"), Buffer.from(GPS_MARKER, "ascii")]);
  const header = 8;
  const ifd0Size = 2 + 12 * 1 + 4;
  const gpsIfdOffset = header + ifd0Size;
  const gpsIfdSize = 2 + 12 * 3 + 4;
  const latOffset = gpsIfdOffset + gpsIfdSize;
  const areaOffset = latOffset + 24;
  const buf = Buffer.alloc(areaOffset + area.length);
  buf.write("II", 0, "ascii");
  buf.writeUInt16LE(42, 2);
  buf.writeUInt32LE(8, 4);
  // IFD0
  let o = header;
  buf.writeUInt16LE(1, o);
  o += 2;
  buf.writeUInt16LE(0x8825, o); // GPSInfo
  buf.writeUInt16LE(4, o + 2); // LONG
  buf.writeUInt32LE(1, o + 4);
  buf.writeUInt32LE(gpsIfdOffset, o + 8);
  o += 12;
  buf.writeUInt32LE(0, o);
  // GPS IFD
  o = gpsIfdOffset;
  buf.writeUInt16LE(3, o);
  o += 2;
  buf.writeUInt16LE(0x0001, o); // GPSLatitudeRef
  buf.writeUInt16LE(2, o + 2); // ASCII
  buf.writeUInt32LE(2, o + 4);
  buf.write("N\0", o + 8, "ascii");
  o += 12;
  buf.writeUInt16LE(0x0002, o); // GPSLatitude
  buf.writeUInt16LE(5, o + 2); // RATIONAL
  buf.writeUInt32LE(3, o + 4);
  buf.writeUInt32LE(latOffset, o + 8);
  o += 12;
  buf.writeUInt16LE(0x001c, o); // GPSAreaInformation
  buf.writeUInt16LE(7, o + 2); // UNDEFINED
  buf.writeUInt32LE(area.length, o + 4);
  buf.writeUInt32LE(areaOffset, o + 8);
  o += 12;
  buf.writeUInt32LE(0, o);
  // 緯度 35/1, 41/1, 2232/100
  [35, 1, 41, 1, 2232, 100].forEach((v, i) => buf.writeUInt32LE(v, latOffset + i * 4));
  area.copy(buf, areaOffset);
  return buf;
}

/** 16×16の単色のJPEG（Pillowで作ったもの）。 */
const BASE_JPEG = Buffer.from(
  "/9j/4AAQSkZJRgABAQAAAQABAAD/2wBDAAYEBQYFBAYGBQYHBwYIChAKCgkJChQODwwQFxQYGBcUFhYaHSUfGhsjHBYWICwgIyYnKSopGR8tMC0oMCUoKSj/2wBDAQcHBwoIChMKChMoGhYaKCgoKCgoKCgoKCgoKCgoKCgoKCgoKCgoKCgoKCgoKCgoKCgoKCgoKCgoKCgoKCgoKCj/wAARCAAQABADASIAAhEBAxEB/8QAHwAAAQUBAQEBAQEAAAAAAAAAAAECAwQFBgcICQoL/8QAtRAAAgEDAwIEAwUFBAQAAAF9AQIDAAQRBRIhMUEGE1FhByJxFDKBkaEII0KxwRVS0fAkM2JyggkKFhcYGRolJicoKSo0NTY3ODk6Q0RFRkdISUpTVFVWV1hZWmNkZWZnaGlqc3R1dnd4eXqDhIWGh4iJipKTlJWWl5iZmqKjpKWmp6ipqrKztLW2t7i5usLDxMXGx8jJytLT1NXW19jZ2uHi4+Tl5ufo6erx8vP09fb3+Pn6/8QAHwEAAwEBAQEBAQEBAQAAAAAAAAECAwQFBgcICQoL/8QAtREAAgECBAQDBAcFBAQAAQJ3AAECAxEEBSExBhJBUQdhcRMiMoEIFEKRobHBCSMzUvAVYnLRChYkNOEl8RcYGRomJygpKjU2Nzg5OkNERUZHSElKU1RVVldYWVpjZGVmZ2hpanN0dXZ3eHl6goOEhYaHiImKkpOUlZaXmJmaoqOkpaanqKmqsrO0tba3uLm6wsPExcbHyMnK0tPU1dbX2Nna4uPk5ebn6Onq8vP09fb3+Pn6/9oADAMBAAIRAxEAPwDgKKKK+eP2E//Z",
  "base64",
);

/** JPEG。exif を渡すと、SOI の直後に APP1（Exif）として埋め込む。 */
export function jpegImage(options: { exif?: Buffer } = {}): Buffer {
  if (!options.exif) return Buffer.from(BASE_JPEG);
  const payload = Buffer.concat([Buffer.from("Exif\0\0", "latin1"), options.exif]);
  const app1 = Buffer.alloc(4);
  app1.writeUInt16BE(0xffe1, 0);
  app1.writeUInt16BE(payload.length + 2, 2);
  return Buffer.concat([BASE_JPEG.subarray(0, 2), app1, payload, BASE_JPEG.subarray(2)]);
}

/** ファイルの中に、位置情報（GPS IFD を指す GPSInfo タグ）を持つEXIFがあるか。 */
export function hasGpsExif(bytes: Buffer): boolean {
  for (let i = 0; i + 8 <= bytes.length; i++) {
    const le = bytes[i] === 0x49 && bytes[i + 1] === 0x49 && bytes[i + 2] === 0x2a && bytes[i + 3] === 0x00;
    const be = bytes[i] === 0x4d && bytes[i + 1] === 0x4d && bytes[i + 2] === 0x00 && bytes[i + 3] === 0x2a;
    if (!le && !be) continue;
    const u16 = (o: number) => (le ? bytes.readUInt16LE(o) : bytes.readUInt16BE(o));
    const u32 = (o: number) => (le ? bytes.readUInt32LE(o) : bytes.readUInt32BE(o));
    try {
      const ifd0 = i + u32(i + 4);
      const count = u16(ifd0);
      if (count > 500) continue;
      for (let k = 0; k < count; k++) {
        if (u16(ifd0 + 2 + k * 12) === 0x8825) return true;
      }
    } catch {
      // TIFFのヘッダーに見えただけの別のデータ
    }
  }
  return false;
}

// ---------------------------------------------------------------------------
// 許可していない形式
// ---------------------------------------------------------------------------

/** 16×16のAVIF（Pillowで作ったもの）。中身はAVIFとして正しい。 */
export const AVIF_IMAGE = Buffer.from(
  "AAAAIGZ0eXBhdmlmAAAAAGF2aWZtaWYxbWlhZk1BMUIAAADrbWV0YQAAAAAAAAAhaGRscgAAAAAAAAAAcGljdAAAAAAAAAAAAAAAAAAAAAAOcGl0bQAAAAAAAQAAAB5pbG9jAAAAAEQAAAEAAQAAAAEAAAETAAAAKgAAAChpaW5mAAAAAAABAAAAGmluZmUCAAAAAAEAAGF2MDFDb2xvcgAAAABqaXBycAAAAEtpcGNvAAAAFGlzcGUAAAAAAAAAEAAAABAAAAAQcGl4aQAAAAADCAgIAAAADGF2MUOBAAwAAAAAE2NvbHJuY2x4AAEADQAGgAAAABdpcG1hAAAAAAAAAAEAAQQBAoMEAAAAMm1kYXQSAAoJGAz/2iAhoNCAMhsUx4eGZQIIIJ5QAAAAOtxBfmJOg8cliODq/Bo=",
  "base64",
);

export const SVG_WITH_SCRIPT = Buffer.from(
  '<?xml version="1.0"?><svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><script>alert(document.domain)</script><rect width="10" height="10"/></svg>',
  "utf8",
);

export const HTML_PAGE = Buffer.from("<!doctype html><html><body><script>alert(document.cookie)</script></body></html>", "utf8");

export const PDF_STUB = Buffer.from("%PDF-1.4\n1 0 obj<<>>endobj\ntrailer<<>>\n%%EOF\n", "latin1");

/** GIFのヘッダーで始まり、JavaScriptとしても読める（画像とスクリプトを兼ねる）ファイル。 */
export const GIF_JS_POLYGLOT = Buffer.concat([
  Buffer.from("GIF89a", "ascii"),
  Buffer.from([0x2f, 0x2a, 0x01, 0x00, 0x80, 0x00, 0x00]),
  Buffer.from([0xff, 0xff, 0xff, 0x00, 0x00, 0x00]),
  Buffer.from([0x2c, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x01, 0x00, 0x00, 0x02, 0x02, 0x44, 0x01, 0x00, 0x3b]),
  Buffer.from("*/=1;alert(document.domain);/*", "ascii"),
]);
