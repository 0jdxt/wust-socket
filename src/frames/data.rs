use super::Opcode;
use crate::{MAX_FRAME_PAYLOAD, role::Role};

// -- SLOW PATH --
// DataFrames may be fragmented or very large hence they need extra processing compared to
// ControlFrames
pub(crate) struct DataFrame<'a> {
    opcode: Opcode,
    payload: &'a [u8],
    role: Role,
}

impl<'a> DataFrame<'a> {
    pub(crate) fn new(payload: &'a [u8], opcode: Opcode, role: Role) -> Self {
        Self {
            opcode,
            payload,
            role,
        }
    }

    pub(crate) fn encode(self) -> Vec<Vec<u8>> {
        let mut payload = self.payload;
        let mut first = true;
        let mut chunks = vec![];

        while !payload.is_empty() {
            let mut buf = vec![];

            let chunk_len = payload.len().min(MAX_FRAME_PAYLOAD);

            // Set OPCODE and FIN
            let opcode = if first { self.opcode } else { Opcode::Cont };
            let is_fin = chunk_len == payload.len();
            buf.push(if is_fin { 0x80 } else { 0 } | opcode as u8);

            // push LEN
            // print!("encoding: {opcode:?} ({chunk_len:>3})");
            match chunk_len {
                0..=125 => {
                    // print!("  u8 ");
                    buf.push(u8::try_from(chunk_len).unwrap());
                }
                126..=65535 => {
                    // print!(" u16 ");
                    buf.push(126);
                    buf.extend_from_slice(&u16::try_from(chunk_len).unwrap().to_be_bytes());
                }
                _ => {
                    // print!(" u64 ");
                    buf.push(127);
                    buf.extend_from_slice(&u64::try_from(chunk_len).unwrap().to_be_bytes());
                }
            }
            // println!("{buf:?}");

            let chunk = &payload[..chunk_len];

            // Clients must SEND masked
            if self.role.is_client() {
                // set MASK bit
                buf[1] |= 0x80;
                // get random bytes and push to buf
                let mut mask_key = [0u8; 4];
                rand::fill(&mut mask_key);
                buf.extend_from_slice(&mask_key);
                // mask bytes
                for (i, &b) in chunk.iter().enumerate() {
                    buf.push(b ^ mask_key[i % 4]);
                }
            } else {
                buf.extend_from_slice(chunk);
            }

            chunks.push(buf);
            payload = &payload[chunk_len..];
            first = false;
        }

        chunks
    }
}
