use std::collections::BTreeMap;

const MAX_DEPTH: usize = 128;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    Bytes(Vec<u8>),
    Int(i64),
    List(Vec<Value>),
    Dict(BTreeMap<Vec<u8>, Value>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    UnexpectedEnd,
    UnexpectedByte(u8),
    BadInteger,
    BadLength,
    NonStringKey,
    UnsortedOrDuplicateKey,
    TrailingData,
    TooDeep,
}

pub fn decode(input: &[u8]) -> Result<Value, Error> {
    let mut parser = Parser { input, pos: 0 };
    let value = parser.value(0)?;
    if parser.pos != input.len() {
        return Err(Error::TrailingData);
    }
    Ok(value)
}

pub(crate) struct Parser<'a> {
    pub(crate) input: &'a [u8],
    pub(crate) pos: usize,
}

impl<'a> Parser<'a> {
    pub(crate) fn new(input: &'a [u8]) -> Self {
        Parser { input, pos: 0 }
    }

    fn peek(&self) -> Result<u8, Error> {
        self.input.get(self.pos).copied().ok_or(Error::UnexpectedEnd)
    }

    fn take(&mut self) -> Result<u8, Error> {
        let b = self.peek()?;
        self.pos += 1;
        Ok(b)
    }

    pub(crate) fn value(&mut self, depth: usize) -> Result<Value, Error> {
        if depth > MAX_DEPTH {
            return Err(Error::TooDeep);
        }
        match self.peek()? {
            b'i' => self.integer(),
            b'l' => self.list(depth),
            b'd' => self.dict(depth),
            b'0'..=b'9' => self.string(),
            other => Err(Error::UnexpectedByte(other)),
        }
    }

    fn integer(&mut self) -> Result<Value, Error> {
        self.take()?;
        let negative = self.peek()? == b'-';
        if negative {
            self.take()?;
        }
        let digits_start = self.pos;
        while self.peek()?.is_ascii_digit() {
            self.take()?;
        }
        let digits = &self.input[digits_start..self.pos];
        if self.take()? != b'e' {
            return Err(Error::BadInteger);
        }
        if digits.is_empty() {
            return Err(Error::BadInteger);
        }
        if digits.len() > 1 && digits[0] == b'0' {
            return Err(Error::BadInteger);
        }
        if negative && digits == b"0" {
            return Err(Error::BadInteger);
        }
        let text = std::str::from_utf8(digits).map_err(|_| Error::BadInteger)?;
        let magnitude: i128 = text.parse().map_err(|_| Error::BadInteger)?;
        let signed = if negative { -magnitude } else { magnitude };
        i64::try_from(signed).map(Value::Int).map_err(|_| Error::BadInteger)
    }

    fn string(&mut self) -> Result<Value, Error> {
        let digits_start = self.pos;
        while self.peek()?.is_ascii_digit() {
            self.take()?;
        }
        let digits = &self.input[digits_start..self.pos];
        if self.take()? != b':' {
            return Err(Error::BadLength);
        }
        if digits.len() > 1 && digits[0] == b'0' {
            return Err(Error::BadLength);
        }
        let text = std::str::from_utf8(digits).map_err(|_| Error::BadLength)?;
        let len: usize = text.parse().map_err(|_| Error::BadLength)?;
        if self.input.len() - self.pos < len {
            return Err(Error::UnexpectedEnd);
        }
        let bytes = self.input[self.pos..self.pos + len].to_vec();
        self.pos += len;
        Ok(Value::Bytes(bytes))
    }

    fn list(&mut self, depth: usize) -> Result<Value, Error> {
        self.take()?;
        let mut items = Vec::new();
        loop {
            if self.peek()? == b'e' {
                self.take()?;
                return Ok(Value::List(items));
            }
            items.push(self.value(depth + 1)?);
        }
    }

    fn dict(&mut self, depth: usize) -> Result<Value, Error> {
        self.take()?;
        let mut map = BTreeMap::new();
        let mut last_key: Option<Vec<u8>> = None;
        loop {
            if self.peek()? == b'e' {
                self.take()?;
                return Ok(Value::Dict(map));
            }
            let key = match self.value(depth + 1)? {
                Value::Bytes(k) => k,
                _ => return Err(Error::NonStringKey),
            };
            if let Some(prev) = &last_key {
                if *prev >= key {
                    return Err(Error::UnsortedOrDuplicateKey);
                }
            }
            let item = self.value(depth + 1)?;
            last_key = Some(key.clone());
            map.insert(key, item);
        }
    }
}
