use sha1::{Digest, Sha1};

pub fn piece_digest(data: &[u8]) -> [u8; 20] {
    Sha1::digest(data).into()
}

pub fn verify_piece(data: &[u8], expected: &[u8; 20]) -> bool {
    piece_digest(data) == *expected
}
