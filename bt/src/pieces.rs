#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bitfield {
    bits: Vec<u8>,
    pieces: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BitfieldError {
    WrongLength,
    SpareBitsSet,
}

impl Bitfield {
    pub fn new(pieces: u32) -> Self {
        Bitfield {
            bits: vec![0u8; pieces.div_ceil(8) as usize],
            pieces,
        }
    }

    pub fn from_bytes(bytes: &[u8], pieces: u32) -> Result<Self, BitfieldError> {
        if bytes.len() != pieces.div_ceil(8) as usize {
            return Err(BitfieldError::WrongLength);
        }
        let spare = (8 - (pieces % 8)) % 8;
        if spare > 0 {
            let last = bytes[bytes.len() - 1];
            if last & ((1u16 << spare) - 1) as u8 != 0 {
                return Err(BitfieldError::SpareBitsSet);
            }
        }
        Ok(Bitfield {
            bits: bytes.to_vec(),
            pieces,
        })
    }

    pub fn pieces(&self) -> u32 {
        self.pieces
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.bits
    }

    pub fn has(&self, index: u32) -> bool {
        if index >= self.pieces {
            return false;
        }
        self.bits[(index / 8) as usize] & (0x80 >> (index % 8)) != 0
    }

    pub fn set(&mut self, index: u32) {
        if index < self.pieces {
            self.bits[(index / 8) as usize] |= 0x80 >> (index % 8);
        }
    }

    pub fn clear(&mut self, index: u32) {
        if index < self.pieces {
            self.bits[(index / 8) as usize] &= !(0x80 >> (index % 8));
        }
    }

    pub fn count_set(&self) -> u32 {
        self.bits.iter().map(|b| b.count_ones()).sum()
    }

    pub fn is_complete(&self) -> bool {
        self.count_set() == self.pieces
    }

    pub fn missing(&self) -> impl Iterator<Item = u32> + '_ {
        (0..self.pieces).filter(move |i| !self.has(*i))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Availability {
    counts: Vec<u32>,
}

impl Availability {
    pub fn new(pieces: u32) -> Self {
        Availability {
            counts: vec![0; pieces as usize],
        }
    }

    pub fn count(&self, index: u32) -> u32 {
        self.counts.get(index as usize).copied().unwrap_or(0)
    }

    pub fn add_have(&mut self, index: u32) {
        if let Some(c) = self.counts.get_mut(index as usize) {
            *c += 1;
        }
    }

    pub fn add_bitfield(&mut self, bitfield: &Bitfield) {
        for index in 0..bitfield.pieces() {
            if bitfield.has(index) {
                self.add_have(index);
            }
        }
    }

    pub fn remove_bitfield(&mut self, bitfield: &Bitfield) {
        for index in 0..bitfield.pieces() {
            if bitfield.has(index) {
                if let Some(c) = self.counts.get_mut(index as usize) {
                    *c = c.saturating_sub(1);
                }
            }
        }
    }
}
