use vni::{Author, Error, Payload, Pixels, VniFile};

fn author(n: &str) -> Author {
    Author { name: n.into(), email: format!("{}@example.com", n.to_lowercase()) }
}

/// Deterministic noisy-ish gradient image.
fn gradient(w: u32, h: u32) -> Pixels {
    let mut data = Vec::new();
    for y in 0..h {
        for x in 0..w {
            data.extend_from_slice(&[(x * 7 % 256) as u8, (y * 5 % 256) as u8, ((x ^ y) % 256) as u8, 255]);
        }
    }
    Pixels::new(w, h, data).unwrap()
}

/// Incompressible-ish pseudo random image (xorshift).
fn noise(w: u32, h: u32) -> Pixels {
    let mut x = 0x2545_f491_4f6c_dd1du64;
    let data = (0..w * h * 4)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            (x >> 24) as u8
        })
        .collect();
    Pixels::new(w, h, data).unwrap()
}

fn paint(img: &Pixels, x0: u32, y0: u32, size: u32, rgba: [u8; 4]) -> Pixels {
    let mut out = img.clone();
    for y in y0..y0 + size {
        for x in x0..x0 + size {
            let i = ((y * img.width + x) * 4) as usize;
            out.data[i..i + 4].copy_from_slice(&rgba);
        }
    }
    out
}

#[test]
fn history_roundtrips_exactly() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a.vni");
    let v0 = gradient(64, 48);
    let v1 = paint(&v0, 10, 10, 8, [255, 0, 0, 255]);
    let v2 = paint(&v1, 30, 20, 12, [0, 0, 255, 128]); // semi-transparent

    let mut f = VniFile::create(&path, &v0, author("Ada"), "base".into(), 1000).unwrap();
    f.commit(&v1, author("Grace"), "red square".into(), 2000, false).unwrap();
    f.commit(&v2, author("Linus"), "blue square".into(), 3000, false).unwrap();

    // Reopen from disk so we test the serialised form.
    let f = VniFile::open(&path).unwrap();
    assert_eq!(f.commits().len(), 3);
    assert_eq!(f.checkout(0).unwrap(), v0);
    assert_eq!(f.checkout(1).unwrap(), v1);
    assert_eq!(f.checkout(2).unwrap(), v2);
    assert_eq!(f.commits()[1].author, author("Grace"));
    assert_eq!(f.commits()[2].message, "blue square");
    assert!(matches!(f.commits()[1].payload, Payload::Delta(_)));
    f.verify().unwrap();
}

#[test]
fn small_edit_is_far_smaller_than_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a.vni");
    let v0 = noise(256, 256);
    let v1 = paint(&v0, 5, 5, 4, [1, 2, 3, 255]);
    let mut f = VniFile::create(&path, &v0, author("Ada"), "base".into(), 0).unwrap();
    f.commit(&v1, author("Ada"), "tweak".into(), 1, false).unwrap();
    let base_len = f.commits()[0].payload.bytes().len();
    let delta_len = f.commits()[1].payload.bytes().len();
    assert!(delta_len * 100 < base_len, "delta {delta_len} vs base {base_len}");
}

#[test]
fn dimension_change_stores_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a.vni");
    let v0 = gradient(16, 16);
    let v1 = gradient(20, 10);
    let mut f = VniFile::create(&path, &v0, author("Ada"), "base".into(), 0).unwrap();
    f.commit(&v1, author("Ada"), "resize".into(), 1, false).unwrap();
    let f = VniFile::open(&path).unwrap();
    assert!(matches!(f.commits()[1].payload, Payload::Full(_)));
    assert_eq!(f.checkout(0).unwrap(), v0);
    assert_eq!(f.checkout(1).unwrap(), v1);
}

#[test]
fn identical_commit_rejected_unless_allowed() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a.vni");
    let v0 = gradient(8, 8);
    let mut f = VniFile::create(&path, &v0, author("Ada"), "base".into(), 0).unwrap();
    assert!(matches!(f.commit(&v0, author("Ada"), "noop".into(), 1, false), Err(Error::NothingToCommit(_))));
    f.commit(&v0, author("Ada"), "noop".into(), 1, true).unwrap();
    assert_eq!(VniFile::open(&path).unwrap().commits().len(), 2);
}

#[test]
fn create_refuses_to_overwrite() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a.vni");
    let v0 = gradient(4, 4);
    VniFile::create(&path, &v0, author("Ada"), "base".into(), 0).unwrap();
    assert!(VniFile::create(&path, &v0, author("Ada"), "again".into(), 0).is_err());
}

#[test]
fn revisions_resolve() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a.vni");
    let v0 = gradient(8, 8);
    let mut f = VniFile::create(&path, &v0, author("Ada"), "base".into(), 0).unwrap();
    f.commit(&paint(&v0, 0, 0, 2, [9, 9, 9, 255]), author("Ada"), "one".into(), 1, false).unwrap();
    assert_eq!(f.resolve("HEAD").unwrap(), 1);
    assert_eq!(f.resolve("head~1").unwrap(), 0);
    assert_eq!(f.resolve("BASE").unwrap(), 0);
    assert_eq!(f.resolve("#1").unwrap(), 1);
    let id = f.commits()[1].hex_id();
    assert_eq!(f.resolve(&id[..6]).unwrap(), 1);
    assert!(f.resolve("HEAD~5").is_err());
    assert!(f.resolve("#9").is_err());
    assert!(f.resolve("zzzz").is_err());
}

#[test]
fn corruption_is_detected() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a.vni");
    let v0 = gradient(32, 32);
    let mut f = VniFile::create(&path, &v0, author("Ada"), "base".into(), 0).unwrap();
    f.commit(&paint(&v0, 3, 3, 5, [7, 7, 7, 255]), author("Ada"), "edit".into(), 1, false).unwrap();
    let good = std::fs::read(&path).unwrap();

    // Flip a byte in the middle of the file.
    let mut bad = good.clone();
    let mid = bad.len() / 2;
    bad[mid] ^= 0xff;
    std::fs::write(&path, &bad).unwrap();
    assert!(VniFile::open(&path).and_then(|f| f.verify()).is_err());

    // Truncate mid-record.
    std::fs::write(&path, &good[..good.len() - 7]).unwrap();
    assert!(matches!(VniFile::open(&path), Err(Error::Corrupt(_))));

    // Wrong magic.
    std::fs::write(&path, b"not a vni file at all").unwrap();
    assert!(matches!(VniFile::open(&path), Err(Error::BadMagic)));
}

#[test]
fn author_parsing() {
    let a = Author::parse("Ada Lovelace <ada@example.com>").unwrap();
    assert_eq!((a.name.as_str(), a.email.as_str()), ("Ada Lovelace", "ada@example.com"));
    assert!(Author::parse("no email").is_err());
    assert!(Author::parse("<only@email>").is_err());
}

#[test]
fn diff_stats_bbox() {
    let a = gradient(10, 10);
    let b = paint(&a, 2, 3, 2, [0, 0, 0, 0]);
    let s = a.diff_stats(&b).unwrap();
    assert_eq!(s.bbox, Some((2, 3, 3, 4)));
    assert!(s.changed_pixels <= 4 && s.changed_pixels > 0);
    assert_eq!(a.diff_stats(&a).unwrap().bbox, None);
}

#[test]
fn delta_handles_scattered_and_full_changes() {
    let a = noise(37, 29);
    // Scattered single-byte changes, including first and last byte.
    let mut b = a.clone();
    for i in (0..b.data.len()).step_by(97) {
        b.data[i] = b.data[i].wrapping_add(1 + (i % 200) as u8);
    }
    let last = b.data.len() - 1;
    b.data[last] ^= 0x55;
    assert_eq!(a.apply_delta(&a.encode_delta(&b).unwrap()).unwrap(), b);
    // Everything changed.
    let c = noise(37, 29).diff_image(&a).unwrap();
    assert_eq!(a.apply_delta(&a.encode_delta(&c).unwrap()).unwrap(), c);
    // Identical images.
    assert_eq!(a.apply_delta(&a.encode_delta(&a).unwrap()).unwrap(), a);
}

#[test]
fn malformed_delta_is_rejected_not_panicking() {
    use flate2::{write::ZlibEncoder, Compression};
    use std::io::Write;
    let a = noise(4, 4);
    let z = |raw: &[u8]| {
        let mut e = ZlibEncoder::new(Vec::new(), Compression::fast());
        e.write_all(raw).unwrap();
        e.finish().unwrap()
    };
    assert!(a.apply_delta(&z(&[0, 200, 1])).is_err()); // literal beyond end
    assert!(a.apply_delta(&z(&[100, 1, 1])).is_err()); // skip beyond end
    assert!(a.apply_delta(&z(&[0x80])).is_err()); // truncated varint
    assert!(a.apply_delta(b"garbage").is_err()); // not zlib
}
