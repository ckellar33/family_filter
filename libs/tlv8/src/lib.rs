use bytes::{BufMut, BytesMut};

#[derive(Clone, Copy, Debug, PartialEq)]
#[repr(u8)]
pub enum T {
    Method = 0x00,
    Identifier = 0x01,
    Salt = 0x02,
    PublicKey = 0x03,
    Proof = 0x04,
    EncryptedData = 0x05,
    SeqNum = 0x06,
    Error = 0x07,
    RetryDelay = 0x08,
    Certificates = 0x09,
    Signature = 0x0A,
    Permissions = 0x0B,
    FragmentData = 0x0C,
    FragmentLast = 0x0D,
    /// Companion / Apple-internal: OPACK device info in Pair-Setup M5.
    Name = 0x11,
    Flags = 0x13,
    Separator = 0xFF,
}

impl From<u8> for T {
    fn from(value: u8) -> Self {
        match value {
            0x00 => T::Method,
            0x01 => T::Identifier,
            0x02 => T::Salt,
            0x03 => T::PublicKey,
            0x04 => T::Proof,
            0x05 => T::EncryptedData,
            0x06 => T::SeqNum,
            0x07 => T::Error,
            0x08 => T::RetryDelay,
            0x09 => T::Certificates,
            0x0A => T::Signature,
            0x0B => T::Permissions,
            0x0C => T::FragmentData,
            0x0D => T::FragmentLast,
            0x11 => T::Name,
            0x13 => T::Flags,
            0xFF => T::Separator,
            _ => T::Separator,
        }
    }
}

#[derive(Clone, Copy, Debug)]
#[repr(u8)]
pub enum Method {
    PairSetup = 0x00,
    PairSetupWithAuth = 0x01,
    PairVerify = 0x02,
    AddPairing = 0x03,
    RemovePairing = 0x04,
    ListPairings = 0x05,
}

#[derive(Clone, Copy, Debug)]
#[repr(u8)]
pub enum State { M1=1, M2, M3, M4, M5, M6 }

#[derive(Clone, Copy, Debug)]
#[repr(u8)]
pub enum Error { Unknown=0x01, Authentication=0x02, Busy=0x06, MaxTries=0x09 }

#[derive(Default, Debug)]
pub struct Tlv8(Vec<(u8, Vec<u8>)>);

impl Tlv8 {
    pub fn new() -> Self { Self::default() }
    pub fn add(mut self, t: T, v: impl Into<Vec<u8>>) -> Self { self.0.push((t as u8, v.into())); self }
    pub fn add_u8(self, t: T, v: u8) -> Self { self.add(t, vec![v]) }
    pub fn encode(&self) -> BytesMut {
        let mut out = BytesMut::new();
        for (t, v) in &self.0 {
            if v.len() <= 255 {
                out.put_u8(*t);
                out.put_u8(v.len() as u8);
                out.extend_from_slice(v);
            } else {
                // fragment into 255-byte chunks
                let mut i = 0;
                while i < v.len() {
                    let end = (i + 255).min(v.len());
                    out.put_u8(*t);
                    out.put_u8((end - i) as u8);
                    out.extend_from_slice(&v[i..end]);
                    i = end;
                }
            }
        }
        out
    }
    pub fn decode(mut data: &[u8]) -> anyhow::Result<Vec<(T, Vec<u8>)>> {
        let mut out: Vec<(T, Vec<u8>)> = vec![];
        while !data.is_empty() {
            if data.len() < 2 { break; }
            let t = data[0]; let len = data[1] as usize;
            data = &data[2..];
            if data.len() < len { anyhow::bail!("truncated TLV8"); }
            let val = data[..len].to_vec();
            data = &data[len..];
            // reassemble contiguous fragments of same type
            if let Some((last_t, last_v)) = out.last_mut() {
                if *last_t == t.into() {
                    last_v.extend_from_slice(&val);
                    continue;
                }
            }
            out.push((t.into(), val));
        }
        Ok(out)
    }
    pub fn get<'a>(items: &'a[(u8, Vec<u8>)], t: T) -> Option<&'a [u8]> {
        items.iter().find(|(tt,_)| *tt==t as u8).map(|(_,v)| v.as_slice())
    }
}
