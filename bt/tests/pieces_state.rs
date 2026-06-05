use bt::pieces::{Availability, Bitfield, BitfieldError};

#[test]
fn empty_bitfield_has_nothing() {
    let bits = Bitfield::new(10);
    assert_eq!(bits.count_set(), 0);
    assert!(!bits.is_complete());
    assert!(!bits.has(0));
    assert_eq!(bits.missing().count(), 10);
}

#[test]
fn set_and_query_across_byte_boundary() {
    let mut bits = Bitfield::new(10);
    bits.set(0);
    bits.set(7);
    bits.set(8);
    bits.set(9);
    assert!(bits.has(0) && bits.has(7) && bits.has(8) && bits.has(9));
    assert!(!bits.has(1));
    assert_eq!(bits.count_set(), 4);
    assert_eq!(bits.as_bytes(), &[0b1000_0001, 0b1100_0000]);
}

#[test]
fn out_of_range_is_ignored() {
    let mut bits = Bitfield::new(3);
    bits.set(3);
    assert_eq!(bits.count_set(), 0);
    assert!(!bits.has(100));
}

#[test]
fn completion_detected() {
    let mut bits = Bitfield::new(9);
    for i in 0..9 {
        bits.set(i);
    }
    assert!(bits.is_complete());
    assert_eq!(bits.missing().count(), 0);
}

#[test]
fn from_bytes_validates_length() {
    assert_eq!(Bitfield::from_bytes(&[0], 9), Err(BitfieldError::WrongLength));
    assert_eq!(Bitfield::from_bytes(&[0, 0, 0], 9), Err(BitfieldError::WrongLength));
    assert!(Bitfield::from_bytes(&[0, 0], 9).is_ok());
}

#[test]
fn from_bytes_rejects_spare_bits() {
    assert_eq!(
        Bitfield::from_bytes(&[0xff, 0xff], 9),
        Err(BitfieldError::SpareBitsSet)
    );
    let bits = Bitfield::from_bytes(&[0xff, 0x80], 9).unwrap();
    assert!(bits.is_complete());
}

#[test]
fn exact_byte_count_has_no_spare_check() {
    assert!(Bitfield::from_bytes(&[0xff], 8).unwrap().is_complete());
}

#[test]
fn availability_tracks_bitfields_and_haves() {
    let mut avail = Availability::new(4);
    let full = Bitfield::from_bytes(&[0xf0], 4).unwrap();
    let mut partial = Bitfield::new(4);
    partial.set(1);
    avail.add_bitfield(&full);
    avail.add_bitfield(&partial);
    avail.add_have(3);
    assert_eq!(avail.count(0), 1);
    assert_eq!(avail.count(1), 2);
    assert_eq!(avail.count(3), 2);
    avail.remove_bitfield(&partial);
    assert_eq!(avail.count(1), 1);
}

#[test]
fn availability_removal_saturates() {
    let mut avail = Availability::new(2);
    let mut bits = Bitfield::new(2);
    bits.set(0);
    avail.remove_bitfield(&bits);
    assert_eq!(avail.count(0), 0);
}
