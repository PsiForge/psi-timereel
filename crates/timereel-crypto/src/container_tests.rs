#![allow(clippy::unwrap_used)]

use std::io::{Cursor, Read, Seek, SeekFrom};

use proptest::prelude::*;

use crate::*;

fn identity() -> age::x25519::Identity {
    age::x25519::Identity::generate()
}

const THUMB: &[u8] = b"thumb bytes \x00\x01\x02";
const META: &[u8] = br#"{"version":1,"app_name":"unknown"}"#;
const IMAGE: &[u8] = &[7u8; 3000];

fn seal(id: &age::x25519::Identity) -> Vec<u8> {
    seal_capture(&id.to_public().to_string(), THUMB, META, IMAGE).unwrap()
}

fn part_range(bytes: &[u8], part: Part) -> std::ops::Range<usize> {
    let h = Header::parse(bytes).unwrap();
    let start = h.offset(part) as usize;
    start..start + h.len(part) as usize
}

#[test]
fn round_trip_returns_identical_parts() {
    let id = identity();
    let bytes = seal(&id);
    assert_eq!(&bytes[..4], b"TRC1");
    assert_eq!(open_part(&id, &bytes, Part::Thumb).unwrap(), THUMB);
    assert_eq!(open_part(&id, &bytes, Part::Meta).unwrap(), META);
    assert_eq!(open_part(&id, &bytes, Part::Image).unwrap(), IMAGE);
    let all = open_capture(&id, &bytes).unwrap();
    assert_eq!((all.thumb.as_slice(), all.meta.as_slice()), (THUMB, META));
    assert_eq!(all.image, IMAGE);
}

#[test]
fn empty_parts_round_trip() {
    let id = identity();
    let bytes = seal_capture(&id.to_public().to_string(), b"", b"", b"").unwrap();
    let all = open_capture(&id, &bytes).unwrap();
    assert!(all.thumb.is_empty() && all.meta.is_empty() && all.image.is_empty());
}

#[test]
fn sealing_needs_only_the_public_key() {
    // The sealing API takes a `&str`; there is no way to pass a private key.
    let public = identity().to_public().to_string();
    assert!(seal_capture(&public, THUMB, META, IMAGE).is_ok());
    assert!(matches!(
        seal_capture("not a key", THUMB, META, IMAGE),
        Err(ContainerError::Seal(_))
    ));
}

#[test]
fn different_identity_is_wrong_key() {
    let bytes = seal(&identity());
    let other = identity();
    for part in [Part::Thumb, Part::Meta, Part::Image] {
        assert!(matches!(
            open_part(&other, &bytes, part),
            Err(ContainerError::WrongKey)
        ));
    }
    assert!(matches!(
        open_capture(&other, &bytes),
        Err(ContainerError::WrongKey)
    ));
}

#[test]
fn flipping_any_byte_is_an_error_not_a_panic() {
    let id = identity();
    let bytes = seal(&id);
    for i in 0..bytes.len() {
        let mut bad = bytes.clone();
        bad[i] ^= 0x01;
        assert!(open_capture(&id, &bad).is_err(), "flip at {i} was accepted");
    }
}

#[test]
fn flipping_a_byte_in_a_part_fails_that_part() {
    let id = identity();
    let bytes = seal(&id);
    for part in [Part::Thumb, Part::Meta, Part::Image] {
        for i in part_range(&bytes, part) {
            let mut bad = bytes.clone();
            bad[i] ^= 0x80;
            assert!(
                open_part(&id, &bad, part).is_err(),
                "{part:?}: flip at {i} was accepted"
            );
        }
    }
}

#[test]
fn truncation_is_truncated_or_bad_format() {
    let id = identity();
    let bytes = seal(&id);
    for n in 0..bytes.len() {
        let cut = &bytes[..n];
        assert!(
            matches!(
                open_capture(&id, cut),
                Err(ContainerError::Truncated | ContainerError::BadFormat(_))
            ),
            "open_capture at {n}"
        );
        for part in [Part::Thumb, Part::Meta, Part::Image] {
            let r = open_part(&id, cut, part);
            let end = part_range(&bytes, part).end;
            if n < end {
                assert!(
                    matches!(
                        r,
                        Err(ContainerError::Truncated | ContainerError::BadFormat(_))
                    ),
                    "{part:?} at {n}"
                );
            } else {
                assert!(r.is_ok(), "{part:?} at {n} should still open");
            }
        }
    }
}

#[test]
fn extra_bytes_at_the_end_are_bad_format() {
    let id = identity();
    let mut bytes = seal(&id);
    bytes.push(0);
    assert!(matches!(
        open_capture(&id, &bytes),
        Err(ContainerError::BadFormat(_))
    ));
}

#[test]
fn bad_magic_version_and_lengths_are_bad_format() {
    let id = identity();
    let good = seal(&id);

    let mut bad = good.clone();
    bad[0] = b'X';
    assert!(matches!(
        open_part(&id, &bad, Part::Thumb),
        Err(ContainerError::BadFormat(_))
    ));

    let mut bad = good.clone();
    bad[4] = 2;
    assert!(matches!(
        open_part(&id, &bad, Part::Thumb),
        Err(ContainerError::BadFormat(_))
    ));

    // Each length over its limit is rejected before anything is allocated:
    // the buffer is only the 17-byte header.
    for (at, limit) in [(5, MAX_THUMB_LEN), (9, MAX_META_LEN), (13, MAX_IMAGE_LEN)] {
        for len in [limit + 1, u32::MAX, 0] {
            let mut head = good[..HEADER_LEN].to_vec();
            head[at..at + 4].copy_from_slice(&len.to_le_bytes());
            assert!(matches!(
                Header::parse(&head),
                Err(ContainerError::BadFormat(_))
            ));
            assert!(matches!(
                open_part(&id, &head, Part::Thumb),
                Err(ContainerError::BadFormat(_))
            ));
        }
        let mut head = good[..HEADER_LEN].to_vec();
        head[at..at + 4].copy_from_slice(&limit.to_le_bytes());
        assert!(Header::parse(&head).is_ok());
    }
}

#[test]
fn oversized_parts_are_refused_when_sealing() {
    let public = identity().to_public().to_string();
    let big = vec![0u8; MAX_META_LEN as usize];
    assert!(matches!(
        seal_capture(&public, THUMB, &big, IMAGE),
        Err(ContainerError::Seal(_))
    ));
}

#[test]
fn thumb_from_another_capture_is_detected() {
    let id = identity();
    let public = id.to_public().to_string();
    let a = seal_capture(&public, b"thumb A", META, IMAGE).unwrap();
    let b = seal_capture(&public, b"thumb B", META, IMAGE).unwrap();

    // Build a container with B's thumb and A's meta and image.
    let ha = Header::parse(&a).unwrap();
    let thumb_b = &b[part_range(&b, Part::Thumb)];
    let mut mixed = Vec::new();
    mixed.extend_from_slice(&a[..5]);
    mixed.extend_from_slice(&(thumb_b.len() as u32).to_le_bytes());
    mixed.extend_from_slice(&ha.len(Part::Meta).to_le_bytes());
    mixed.extend_from_slice(&ha.len(Part::Image).to_le_bytes());
    mixed.extend_from_slice(thumb_b);
    mixed.extend_from_slice(&a[HEADER_LEN + ha.len(Part::Thumb) as usize..]);

    // Every part is valid on its own; only the id check catches the swap.
    assert!(open_part(&id, &mixed, Part::Thumb).is_ok());
    assert!(matches!(
        open_capture(&id, &mixed),
        Err(ContainerError::Tampered)
    ));
    let ida = open_part_from(&id, &mut Cursor::new(&a), Part::Thumb)
        .unwrap()
        .0;
    let idb = open_part_from(&id, &mut Cursor::new(&b), Part::Thumb)
        .unwrap()
        .0;
    assert_ne!(ida, idb);
}

#[test]
fn part_tags_are_checked() {
    // A meta blob in the thumb slot (same key, same id) has the wrong tag.
    let id = identity();
    let a = seal(&id);
    let h = Header::parse(&a).unwrap();
    let meta = &a[part_range(&a, Part::Meta)];
    let mut swapped = Vec::new();
    swapped.extend_from_slice(&a[..5]);
    swapped.extend_from_slice(&(meta.len() as u32).to_le_bytes());
    swapped.extend_from_slice(&h.len(Part::Meta).to_le_bytes());
    swapped.extend_from_slice(&h.len(Part::Image).to_le_bytes());
    swapped.extend_from_slice(meta);
    swapped.extend_from_slice(&a[part_range(&a, Part::Meta)]);
    swapped.extend_from_slice(&a[part_range(&a, Part::Image)]);
    assert!(matches!(
        open_part(&id, &swapped, Part::Thumb),
        Err(ContainerError::BadFormat("wrong part tag"))
    ));
}

#[test]
fn sealing_twice_gives_different_ciphertexts() {
    let id = identity();
    let public = id.to_public().to_string();
    let a = seal_capture(&public, THUMB, META, IMAGE).unwrap();
    let b = seal_capture(&public, THUMB, META, IMAGE).unwrap();
    assert_ne!(a, b);
    for part in [Part::Thumb, Part::Meta, Part::Image] {
        assert_ne!(a[part_range(&a, part)], b[part_range(&b, part)]);
    }
    // Same plaintext either way.
    assert_eq!(open_part(&id, &b, Part::Image).unwrap(), IMAGE);
}

#[test]
fn caller_chosen_capture_id_is_used_in_every_part() {
    let id = identity();
    let cid = CaptureId([9; CAPTURE_ID_LEN]);
    let bytes =
        seal_capture_with_id(&id.to_public().to_string(), &cid, THUMB, META, IMAGE).unwrap();
    assert_eq!(open_capture(&id, &bytes).unwrap().id, cid);
}

/// A reader that records the largest offset it was asked to read.
struct Spy<R> {
    inner: R,
    max_end: u64,
}

impl<R: Read + Seek> Read for Spy<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.max_end = self.max_end.max(self.inner.stream_position()?);
        Ok(n)
    }
}

impl<R: Read + Seek> Seek for Spy<R> {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        self.inner.seek(pos)
    }
}

#[test]
fn thumbnail_is_read_without_reading_the_image() {
    let id = identity();
    let bytes = seal(&id);
    let h = Header::parse(&bytes).unwrap();
    let mut spy = Spy {
        inner: Cursor::new(bytes.clone()),
        max_end: 0,
    };
    let (_, thumb) = open_part_from(&id, &mut spy, Part::Thumb).unwrap();
    assert_eq!(thumb, THUMB);
    assert_eq!(
        spy.max_end,
        h.offset(Part::Thumb) + u64::from(h.len(Part::Thumb))
    );
    assert!(spy.max_end < h.offset(Part::Image));

    // A buffer holding only the start of the file is enough.
    let prefix = &bytes[..spy.max_end as usize];
    assert_eq!(open_part(&id, prefix, Part::Thumb).unwrap(), THUMB);
}

proptest! {
    #[test]
    fn random_bytes_never_panic(data in proptest::collection::vec(any::<u8>(), 0..2048)) {
        let id = identity();
        let _ = Header::parse(&data);
        for part in [Part::Thumb, Part::Meta, Part::Image] {
            let _ = open_part(&id, &data, part);
        }
        let _ = open_capture(&id, &data);
    }

    #[test]
    fn random_bytes_after_a_valid_header_never_panic(
        lens in (1u32..2048, 1u32..2048, 1u32..2048),
        body in proptest::collection::vec(any::<u8>(), 0..4096),
    ) {
        let id = identity();
        let mut data = b"TRC1\x01".to_vec();
        for l in [lens.0, lens.1, lens.2] {
            data.extend_from_slice(&l.to_le_bytes());
        }
        data.extend_from_slice(&body);
        for part in [Part::Thumb, Part::Meta, Part::Image] {
            let _ = open_part(&id, &data, part);
        }
        let _ = open_capture(&id, &data);
    }

    #[test]
    fn mutated_valid_containers_never_panic(pos in 0usize..4000, val in any::<u8>(), cut in 0usize..4000) {
        let id = identity();
        let mut bytes = seal(&id);
        let i = pos % bytes.len();
        bytes[i] = val;
        bytes.truncate(cut.max(1).min(bytes.len()));
        let _ = open_capture(&id, &bytes);
    }
}
