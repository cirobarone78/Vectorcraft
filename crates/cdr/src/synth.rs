//! Synthetic CorelDRAW streams for tests and fuzz seeds (cargo feature `synth`).
//!
//! These builders write the layouts the reader accepts (libcdr's, see the module docs of
//! [`crate::riff`] and [`crate::geometry`]), so tests can describe a drawing in a few lines. They
//! are **not** a CorelDRAW exporter: nothing written here has been opened in CorelDRAW.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::io::Write;

/// A chunk: fourCC, length, data, and a pad byte after odd-length data.
pub fn chunk(id: &[u8; 4], data: &[u8]) -> Vec<u8> {
    let mut v = id.to_vec();
    v.extend_from_slice(&u32::try_from(data.len()).unwrap().to_le_bytes());
    v.extend_from_slice(data);
    if data.len() % 2 == 1 {
        v.push(0);
    }
    v
}

/// A `RIFF` or `LIST` chunk of `list_type` holding `children` (already-built chunks).
pub fn list(id: &[u8; 4], list_type: &[u8; 4], children: &[Vec<u8>]) -> Vec<u8> {
    let mut body = list_type.to_vec();
    children.iter().for_each(|c| body.extend_from_slice(c));
    chunk(id, &body)
}

pub fn zlib(data: &[u8]) -> Vec<u8> {
    let mut e = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    e.write_all(data).unwrap();
    e.finish().unwrap()
}

/// A `cmpr` list holding `inner` (already-built chunks) and a block table.
pub fn cmpr(inner: &[u8], blocks: &[u32]) -> Vec<u8> {
    let data = zlib(inner);
    let table = zlib(&blocks.iter().flat_map(|b| b.to_le_bytes()).collect::<Vec<_>>());
    let cpng = b"CPng\x01\x00\x04\x00";
    let mut body = b"cmpr".to_vec();
    for n in [data.len() + 8, inner.len(), table.len() + 8, blocks.len() * 4] {
        body.extend_from_slice(&u32::try_from(n).unwrap().to_le_bytes());
    }
    body.extend_from_slice(cpng);
    body.extend_from_slice(&data);
    body.extend_from_slice(cpng);
    body.extend_from_slice(&table);
    chunk(b"LIST", &body)
}

fn le(v: &[u32]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

/// A version 6+ `loda` record with (argument type, payload) arguments.
pub fn loda(chunk_type: u32, args: &[(u32, Vec<u8>)]) -> Vec<u8> {
    let n = u32::try_from(args.len()).unwrap();
    let mut at = 20 + 8 * n;
    let mut offsets = Vec::new();
    for (_, p) in args {
        offsets.push(at);
        at += u32::try_from(p.len()).unwrap();
    }
    let mut v = le(&[at, n, 20, 20 + 4 * n, chunk_type]);
    v.extend_from_slice(&le(&offsets));
    args.iter().rev().for_each(|(t, _)| v.extend_from_slice(&t.to_le_bytes()));
    args.iter().for_each(|(_, p)| v.extend_from_slice(p));
    v
}

/// Units of 1/254000 inch (version 6+ coordinates).
pub const INCH: i32 = 254_000;

/// A version 9 drawing: a 4 × 3 in page (and a master page before it) holding a red triangle
/// with a 1 pt blue outline, and a rectangle (which the reader counts but doesn't translate).
pub fn triangle_riff() -> Vec<u8> {
    let mut red = le(&[1]);
    red.extend_from_slice(&1u16.to_le_bytes());
    red.extend_from_slice(&[0, 0, 5, 0, 0, 0, 0, 0, 0, 0, 0, 0, 255, 0]);
    let mut outl = le(&[2]);
    for x in [0x02u16, 1, 1, 0] {
        outl.extend_from_slice(&x.to_le_bytes());
    }
    outl.extend_from_slice(&(INCH / 72).to_le_bytes());
    outl.extend_from_slice(&100u16.to_le_bytes());
    outl.extend_from_slice(&[0; 6 + 52]);
    outl.extend_from_slice(&[5, 0, 0, 0, 0, 0, 0, 0, 255, 0, 0, 0]);
    outl.extend_from_slice(&[0; 16 + 2 + 22 + 8]);
    let mut tri = 3u16.to_le_bytes().to_vec();
    tri.extend_from_slice(&[0, 0]);
    for (x, y) in [(0, 0), (INCH, 0), (0, INCH)] {
        tri.extend_from_slice(&x.to_le_bytes());
        tri.extend_from_slice(&y.to_le_bytes());
    }
    tri.extend_from_slice(&[0x00, 0x40, 0x48]);
    let mut mcfg = vec![0; 4];
    mcfg.extend_from_slice(&(4 * INCH).to_le_bytes());
    mcfg.extend_from_slice(&(3 * INCH).to_le_bytes());
    let obj = list(b"LIST", b"obj ", &[chunk(b"loda", &loda(0x03, &[(0x14, le(&[1])), (0x0a, le(&[2])), (0x1e, tri)]))]);
    let rect = list(b"LIST", b"obj ", &[chunk(b"loda", &loda(0x01, &[]))]);
    list(
        b"RIFF",
        b"CDR9",
        &[
            list(
                b"LIST",
                b"doc ",
                &[chunk(b"mcfg", &mcfg), list(b"LIST", b"filt", &[chunk(b"fild", &red)]), list(b"LIST", b"otlt", &[chunk(b"outl", &outl)])],
            ),
            list(b"LIST", b"page", &[chunk(b"flgs", &le(&[0x0098_0000]))]),
            list(b"LIST", b"page", &[chunk(b"flgs", &[0; 4]), list(b"LIST", b"layr", &[obj, rect])]),
        ],
    )
}

/// An X4-style package: `riff` as `content/riffData.cdr`, and `metadata.xml` giving `title` and
/// a 4 × 3 in page.
pub fn package(riff: &[u8], title: &str) -> Vec<u8> {
    package_with(riff, title, &[])
}

/// [`package`] with `extra` entries (colour settings and profiles, say).
pub fn package_with(riff: &[u8], title: &str, extra: &[(&str, &[u8])]) -> Vec<u8> {
    let ns = "adobe:ns:meta/"; // brand-ok: the XMP packet's namespace, fixed by the XMP specification
    let meta = format!(
        r#"<x:xmpmeta xmlns:x="{ns}"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
<rdf:Description rdf:about=""><Title>{title}</Title><ProductName>synth</ProductName>
<PageWidth>1016000</PageWidth><PageHeight>762000</PageHeight></rdf:Description></rdf:RDF></x:xmpmeta>"#
    );
    let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    for (name, data) in [("content/riffData.cdr", riff), ("metadata/metadata.xml", meta.as_bytes())].into_iter().chain(extra.iter().copied()) {
        w.start_file(name, opts).unwrap();
        w.write_all(data).unwrap();
    }
    w.finish().unwrap().into_inner()
}

/// A stored (uncompressed) ZIP archive of `entries`, CorelDRAW or not.
pub fn zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (name, data) in entries {
        w.start_file(*name, opts).unwrap();
        w.write_all(data).unwrap();
    }
    w.finish().unwrap().into_inner()
}

/// A version 9 `font` record: `id`, character set `charset`, an eight-bit `name`.
pub fn font(id: u16, charset: u16, name: &str) -> Vec<u8> {
    let mut v = id.to_le_bytes().to_vec();
    v.extend_from_slice(&charset.to_le_bytes());
    v.extend_from_slice(&[0; 14]);
    v.extend_from_slice(name.as_bytes());
    v.push(0);
    v
}

/// A version 9 `stlt` table: style 10 (font 1 at `size_in` inches, alignment `align`, fill
/// `fill`) and style 11, which inherits it and only sets alignment 1 (left).
pub fn stlt(size_in: f64, align: u32, fill: u32) -> Vec<u8> {
    let mut v = le(&[2]); // records
    v.extend_from_slice(&le(&[1, 100, 0, fill])); // fills: id 100 → fild `fill`
    v.extend_from_slice(&le(&[0])); // outlines
    v.extend_from_slice(&le(&[1, 200])); // fonts: id 200
    v.extend_from_slice(&[0; 12]);
    v.extend_from_slice(&1u16.to_le_bytes());
    v.extend_from_slice(&0u16.to_le_bytes());
    v.extend_from_slice(&[0; 8]);
    v.extend_from_slice(&((size_in * f64::from(INCH)) as i32).to_le_bytes());
    v.extend_from_slice(&[0; 12]);
    v.extend_from_slice(&le(&[2, 300, 0, align, 301, 0, 1])); // aligns: 300 → align, 301 → left
    v.extend_from_slice(&le(&[0, 0, 0, 0, 0, 0, 0, 0])); // intervals … set 11s
    for (id, parent, fill, font, align) in [(10, 0, 100, 200, 300), (11, 10, 0, 0, 301)] {
        v.extend_from_slice(&le(&[2, id, parent, 0, 0, 0, fill, 0, font, align, 0, 0, 0]));
    }
    v
}

/// A version 9 `txsm` for text `id`: one paragraph in `style` whose runs are (Windows-1252 text,
/// size override in inches). (Before version 12 the text is one byte a character.)
pub fn txsm(id: u32, style: u32, runs: &[(&str, Option<f64>)]) -> Vec<u8> {
    let mut v = le(&[1]); // frame flag
    v.extend_from_slice(&[0; 0x20]);
    v.extend_from_slice(&le(&[1, id]));
    v.extend_from_slice(&[0; 48]);
    v.extend_from_slice(&le(&[0])); // not on a path
    v.extend_from_slice(&le(&[1, style]));
    v.push(0);
    v.extend_from_slice(&le(&[u32::try_from(runs.len()).unwrap()]));
    for (_, size) in runs {
        v.extend_from_slice(&0u16.to_le_bytes());
        v.push(if size.is_some() { 0x04 } else { 0 });
        v.push(0);
        if let Some(s) = size {
            v.extend_from_slice(&((s * f64::from(INCH)) as i32).to_le_bytes());
        }
    }
    let (mut descs, mut bytes) = (Vec::new(), Vec::new());
    for (i, (text, _)) in runs.iter().enumerate() {
        for c in text.chars() {
            descs.push((2 * u32::try_from(i).unwrap()) << 16);
            bytes.push(u8::try_from(u32::from(c)).unwrap());
        }
    }
    v.extend_from_slice(&le(&[u32::try_from(descs.len()).unwrap()]));
    v.extend_from_slice(&le(&descs));
    v.extend_from_slice(&bytes);
    v.push(0);
    v
}

/// A translation `trfd` (version 9) moving an object by (`x`, `y`) inches.
pub fn trfd_move(x: f64, y: f64) -> Vec<u8> {
    let mut m = le(&[0, 1, 12, 16]);
    m.extend_from_slice(&8u16.to_le_bytes());
    m.extend_from_slice(&[0; 6]);
    for d in [1.0, 0.0, x * f64::from(INCH), 0.0, 1.0, y * f64::from(INCH)] {
        m.extend_from_slice(&f64::to_le_bytes(d));
    }
    m
}

/// A version 9 drawing on a 4 × 3 in page: a 2 × 1 in rectangle with 0.25 in round corners at
/// the page's top-left quarter, a half-inch pie (a quarter cut out), centred artistic text
/// "Hello wörld" (Arial 24 pt, its last word 12 pt) at the page centre, and a 1.5 ×
/// 0.5 in paragraph text frame with "Para" in the inheriting, left-aligned style.
pub fn shapes_riff() -> Vec<u8> {
    let mut red = le(&[1]);
    red.extend_from_slice(&1u16.to_le_bytes());
    red.extend_from_slice(&[0, 0, 5, 0, 0, 0, 0, 0, 0, 0, 0, 0, 255, 0]);
    let mut mcfg = vec![0; 4];
    mcfg.extend_from_slice(&(4 * INCH).to_le_bytes());
    mcfg.extend_from_slice(&(3 * INCH).to_le_bytes());
    let i32s = |v: &[i32]| v.iter().flat_map(|x| x.to_le_bytes()).collect::<Vec<u8>>();
    let rect = i32s(&[2 * INCH, -INCH, INCH / 4, INCH / 4, INCH / 4, INCH / 4]);
    let pie = i32s(&[INCH / 2, INCH / 2, 0, 270_000_000, 1]);
    let obj = |loda_bytes: Vec<u8>, extra: Vec<Vec<u8>>| {
        let mut c = vec![chunk(b"loda", &loda_bytes)];
        c.extend(extra);
        list(b"LIST", b"obj ", &c)
    };
    list(
        b"RIFF",
        b"CDR9",
        &[
            list(
                b"LIST",
                b"doc ",
                &[
                    chunk(b"mcfg", &mcfg),
                    list(b"LIST", b"fntt", &[chunk(b"font", &font(1, 0, "Arial"))]),
                    list(b"LIST", b"filt", &[chunk(b"fild", &red)]),
                    chunk(b"stlt", &stlt(1.0 / 3.0, 2, 1)),
                    chunk(b"txsm", &txsm(7, 10, &[("Hello ", None), ("wörld", Some(1.0 / 6.0))])),
                    chunk(b"txsm", &txsm(8, 11, &[("Para", None)])),
                ],
            ),
            list(
                b"LIST",
                b"page",
                &[
                    chunk(b"flgs", &[0; 4]),
                    list(
                        b"LIST",
                        b"layr",
                        &[
                            obj(loda(0x01, &[(0x14, le(&[1])), (0x1e, rect)]), vec![chunk(b"trfd", &trfd_move(-2.0, 1.5))]),
                            obj(loda(0x02, &[(0x1e, pie)]), vec![]),
                            obj(loda(0x04, &[(0x1e, vec![0; 8]), (0xc8, le(&[10]))]), vec![chunk(b"spnd", &le(&[7]))]),
                            obj(
                                loda(0x06, &[(0x1e, i32s(&[0, INCH * 3 / 2, INCH / 2])), (0xc8, le(&[11]))]),
                                vec![chunk(b"spnd", &le(&[8])), chunk(b"trfd", &trfd_move(0.5, -0.5))],
                            ),
                        ],
                    ),
                ],
            ),
        ],
    )
}

/// An X6 (version 16) package: `content/root.dat` whose triangle object, its fill and the text of
/// an artistic text object ("Ünïcode", Verdana 72 pt, JSON-styled) are 16-byte stubs pointing into
/// `content/data/a.dat` (listed in `content/dataFileList.dat`), on a 4 × 3 in page.
pub fn x6_package() -> Vec<u8> {
    let mut tri = 3u16.to_le_bytes().to_vec();
    tri.extend_from_slice(&[0, 0]);
    for (x, y) in [(0, 0), (INCH, 0), (0, INCH)] {
        tri.extend_from_slice(&x.to_le_bytes());
        tri.extend_from_slice(&y.to_le_bytes());
    }
    tri.extend_from_slice(&[0x00, 0x40, 0x48]);
    let obj = loda(0x03, &[(0x14, le(&[1])), (0x1e, tri)]);
    // Version 13+ solid fill: id, 8 bytes, type 1, 13 bytes, then an RGB colour (red).
    let mut red = le(&[1]);
    red.extend_from_slice(&[0; 8]);
    red.extend_from_slice(&1u16.to_le_bytes());
    red.extend_from_slice(&[0; 13]);
    red.extend_from_slice(&[5, 0, 0, 0, 0, 0, 0, 0, 0, 0, 255, 0]);
    let text = txsm16(crate::riff::CdrVersion(1600), 0, r#"{"character":{"latin":{"font":"Verdana","size":254000}}}"#, "{}", "Ünïcode");
    let mut data = obj.clone();
    data.extend_from_slice(&red);
    data.extend_from_slice(&text);
    let stub = |offset: usize, len: usize| le(&[0, u32::try_from(len).unwrap(), u32::try_from(offset).unwrap(), 0]);
    let mut mcfg = vec![0; 12];
    mcfg.extend_from_slice(&(4 * INCH).to_le_bytes());
    mcfg.extend_from_slice(&(3 * INCH).to_le_bytes());
    let root = list(
        b"RIFF",
        b"CDRG",
        &[
            list(
                b"LIST",
                b"doc ",
                &[
                    chunk(b"mcfg", &mcfg),
                    list(b"LIST", b"filt", &[chunk(b"fild", &stub(obj.len(), red.len()))]),
                    chunk(b"txsm", &stub(obj.len() + red.len(), text.len())),
                ],
            ),
            list(
                b"LIST",
                b"page",
                &[
                    chunk(b"flgs", &[0; 4]),
                    list(
                        b"LIST",
                        b"layr",
                        &[
                            list(b"LIST", b"obj ", &[chunk(b"loda", &stub(0, obj.len()))]),
                            list(b"LIST", b"obj ", &[chunk(b"loda", &loda(0x04, &[(0x1e, vec![0; 8])])), chunk(b"spnd", &le(&[77]))]),
                        ],
                    ),
                ],
            ),
        ],
    );
    zip(&[("content/root.dat", &root), ("content/dataFileList.dat", b"a.dat\n"), ("content/data/a.dat", &data)])
}

/// A version 16+ `txsm` (text id 77): one paragraph in `style`, its JSON style, one run whose JSON sets
/// `run_json`, and UTF-16 `text`.
pub fn txsm16(version: crate::riff::CdrVersion, style: u32, para_json: &str, run_json: &str, text: &str) -> Vec<u8> {
    let utf8 = version >= crate::riff::CdrVersion(1700);
    let json = |j: &str| -> Vec<u8> {
        let (units, bytes): (usize, Vec<u8>) =
            if utf8 { (j.len(), j.as_bytes().to_vec()) } else { (j.encode_utf16().count(), j.encode_utf16().flat_map(u16::to_le_bytes).collect()) };
        let mut v = le(&[u32::try_from(units).unwrap()]);
        v.extend(bytes);
        v
    };
    let mut v = le(&[1]);
    v.extend_from_slice(&[0; 37]);
    v.extend_from_slice(&le(&[1, 77]));
    v.extend_from_slice(&[0; 48]);
    v.extend_from_slice(&le(&[0, 0, 0]));
    v.extend_from_slice(&le(&[1, style]));
    v.extend_from_slice(&[0, 0]);
    v.extend(json(para_json));
    v.extend_from_slice(&le(&[1]));
    v.extend_from_slice(&[0, 0, 1, 0, 0, 0]);
    v.extend(json(run_json));
    let units: Vec<u16> = text.encode_utf16().collect();
    v.extend_from_slice(&le(&[u32::try_from(units.len()).unwrap()]));
    for _ in &units {
        v.extend_from_slice(&1u64.to_le_bytes());
    }
    v.extend_from_slice(&le(&[u32::try_from(units.len() * 2).unwrap()]));
    v.extend(units.iter().flat_map(|u| u.to_le_bytes()));
    v.push(0);
    v
}

/// A version 9 drawing on a 4 × 3 in page with the other object kinds: a pentagon (one corner and
/// its symmetry), a four-point B-spline whose 1 pt outline ends in a closed arrowhead, and a 2 × 2
/// RGB bitmap (red, green / blue, white from the top) in a 1 in square at the page centre.
pub fn more_shapes_riff() -> Vec<u8> {
    let i32s = |v: &[i32]| v.iter().flat_map(|x| x.to_le_bytes()).collect::<Vec<u8>>();
    let mut mcfg = vec![0; 4];
    mcfg.extend_from_slice(&(4 * INCH).to_le_bytes());
    mcfg.extend_from_slice(&(3 * INCH).to_le_bytes());
    // Pentagon: the edge from the top corner to the next, clockwise (y up).
    let a = std::f64::consts::TAU / 5.0;
    let (cx, cy) = ((a.sin() * f64::from(INCH)) as i32, (a.cos() * f64::from(INCH)) as i32);
    let mut corner = 2u16.to_le_bytes().to_vec();
    corner.extend_from_slice(&[0, 0]);
    corner.extend(i32s(&[0, INCH, cx, cy]));
    corner.extend_from_slice(&[0x00, 0x40]);
    let mut symmetry = vec![0; 4];
    symmetry.extend(le(&[5, 1, 1]));
    for d in [1.0f64, 1.0] {
        symmetry.extend_from_slice(&d.to_le_bytes());
    }
    symmetry.extend(i32s(&[0, 0]));
    // Spline: four points, no knot flags (one cubic B-spline piece).
    let mut ppdt = 4u16.to_le_bytes().to_vec();
    ppdt.extend_from_slice(&[0; 4]);
    ppdt.extend(i32s(&[0, 0, INCH, INCH, 2 * INCH, -INCH, 3 * INCH, 0]));
    ppdt.extend(le(&[0, 0, 0, 0]));
    // Arrowhead 5: a closed triangle, points stored y first.
    let mut arrw = le(&[5, 0]);
    arrw.extend_from_slice(&3u16.to_le_bytes());
    arrw.extend_from_slice(&[0; 4]);
    arrw.extend_from_slice(&[0x00, 0x40, 0x48]);
    arrw.push(0);
    arrw.extend(i32s(&[0, 0, INCH / 10, -INCH / 10, -INCH / 10, -INCH / 10]));
    // Outline 2: 1 pt black, no arrow at the start, arrowhead 5 at the end.
    let mut outl = le(&[2]);
    for x in [0x02u16, 0, 0, 0] {
        outl.extend_from_slice(&x.to_le_bytes());
    }
    outl.extend_from_slice(&(INCH / 72).to_le_bytes());
    outl.extend_from_slice(&100u16.to_le_bytes());
    outl.extend_from_slice(&[0; 6 + 52]);
    outl.extend_from_slice(&[5, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    outl.extend_from_slice(&[0; 16 + 2 + 22]);
    outl.extend(le(&[0, 5]));
    // Bitmap 7: 2 × 2, 24-bit, rows bottom first and padded to 4 bytes.
    let mut bmp = le(&[7]);
    bmp.extend_from_slice(&[0; 50]);
    bmp.extend(le(&[1, 0, 2, 2, 0, 24, 0, 16]));
    bmp.extend_from_slice(&[0; 32]);
    bmp.extend_from_slice(&[0, 0, 255, 0, 255, 0, 0, 0]); // bottom: blue, green
    bmp.extend_from_slice(&[255, 0, 0, 255, 255, 255, 0, 0]); // top: blue, white
    let mut place = i32s(&[-INCH / 2, -INCH / 2, INCH / 2, INCH / 2]);
    place.extend_from_slice(&[0; 32]);
    place.extend(le(&[7]));
    let obj = |l: Vec<u8>, extra: Vec<Vec<u8>>| {
        let mut c = vec![chunk(b"loda", &l)];
        c.extend(extra);
        list(b"LIST", b"obj ", &c)
    };
    list(
        b"RIFF",
        b"CDR9",
        &[
            list(
                b"LIST",
                b"doc ",
                &[
                    chunk(b"mcfg", &mcfg),
                    list(b"LIST", b"arrt", &[chunk(b"arrw", &arrw)]),
                    list(b"LIST", b"otlt", &[chunk(b"outl", &outl)]),
                    list(b"LIST", b"bmpt", &[chunk(b"bmp ", &bmp)]),
                ],
            ),
            list(
                b"LIST",
                b"page",
                &[
                    chunk(b"flgs", &[0; 4]),
                    list(
                        b"LIST",
                        b"layr",
                        &[
                            obj(loda(0x14, &[(0x1e, corner), (0x2af8, symmetry)]), vec![]),
                            obj(loda(0x26, &[(0x0a, le(&[2]))]), vec![chunk(b"ppdt", &ppdt)]),
                            obj(loda(0x05, &[(0x1e, place)]), vec![]),
                        ],
                    ),
                ],
            ),
        ],
    )
}
