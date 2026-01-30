use std::marker::PhantomData;

use super::Opcode;
use crate::{role::EncodePolicy, MAX_FRAME_PAYLOAD, MAX_MESSAGE_SIZE};

// -- SLOW PATH --
// DataFrames may be fragmented or very large hence they need extra processing compared to
// ControlFrames
pub(crate) struct DataFrame<'a, P: EncodePolicy> {
    opcode: Opcode,
    payload: &'a [u8],
    _p: PhantomData<P>,
}

impl<'a, P: EncodePolicy> DataFrame<'a, P> {
    pub(crate) fn new(payload: &'a [u8], opcode: Opcode) -> Self {
        Self {
            opcode,
            payload,
            _p: PhantomData,
        }
    }

    pub(crate) fn encode(self) -> Vec<Vec<u8>> {
        let mut payload = self.payload;
        let mut first = true;
        let mut chunks = Vec::with_capacity(MAX_MESSAGE_SIZE.div_ceil(MAX_FRAME_PAYLOAD));

        while !payload.is_empty() {
            let chunk_len = payload.len().min(MAX_FRAME_PAYLOAD);
            let mut buf = Vec::with_capacity(chunk_len + 14);

            // Set OPCODE and FIN
            let opcode = if first { self.opcode } else { Opcode::Cont };
            let is_fin = chunk_len == payload.len();
            buf.push(if is_fin { 0x80 } else { 0 } | opcode as u8);

            // push LEN
            #[allow(clippy::cast_possible_truncation)]
            match chunk_len {
                0..=125 => {
                    buf.push(chunk_len as u8);
                }
                126..=65535 => {
                    buf.push(126);
                    buf.extend_from_slice(&(chunk_len as u16).to_be_bytes());
                }
                _ => {
                    buf.push(127);
                    buf.extend_from_slice(&(chunk_len as u64).to_be_bytes());
                }
            }

            let chunk = &payload[..chunk_len];

            // Clients must SEND masked
            if P::MASK_OUTGOING {
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
