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
        let mut first = true;
        let mut chunks = Vec::with_capacity(MAX_MESSAGE_SIZE.div_ceil(MAX_FRAME_PAYLOAD));
        let mut iter = self.payload.chunks(MAX_FRAME_PAYLOAD).peekable();
        let role = if P::MASK_OUTGOING { "CLI" } else { "SRV" };

        while let Some(chunk) = iter.next() {
            tracing::trace!(
                opcode = ?self.opcode,
                len = chunk.len(),
                "{role} encoding DATA"
            );

            // Set OPCODE and FIN
            let opcode = if first {
                first = false;
                self.opcode
            } else {
                Opcode::Cont
            } as u8;
            let mut buf = Vec::with_capacity(chunk.len() + 14);
            buf.push(if iter.peek().is_none() { 0x80 } else { 0 } | opcode);

            // push LEN
            #[allow(clippy::cast_possible_truncation)]
            match chunk.len() {
                0..=125 => {
                    buf.push(chunk.len() as u8);
                }
                126..=65535 => {
                    buf.push(126);
                    buf.extend_from_slice(&(chunk.len() as u16).to_be_bytes());
                }
                _ => {
                    buf.push(127);
                    buf.extend_from_slice(&(chunk.len() as u64).to_be_bytes());
                }
            }

            // Clients must SEND masked
            if P::MASK_OUTGOING {
                // set MASK bit
                buf[1] |= 0x80;
                // get random bytes and push to buf
                let mut mask_key = [0u8; 4];
                rand::fill(&mut mask_key);
                buf.extend_from_slice(&mask_key);

                // mask bytes
                let start = buf.len();
                buf.extend_from_slice(chunk);
                crate::protocol::mask(&mut buf[start..], mask_key);
            } else {
                buf.extend_from_slice(chunk);
            }

            chunks.push(buf);
        }

        tracing::info!(len = self.payload.len(), "{role} encoded DATA");
        chunks
    }
}

#[cfg(test)]
mod bench {
    extern crate test;
    use paste::paste;
    use test::{black_box, Bencher};

    use super::*;
    use crate::role::*;

    fn make_payload(len: usize) -> Vec<u8> { (0..len).map(|i| i as u8).collect() }

    fn bench_data_frame<P: EncodePolicy>(b: &mut Bencher, payload_len: usize) {
        let payload = make_payload(payload_len);
        b.iter(|| {
            let frame = DataFrame::<P>::new(&payload, Opcode::Text);
            let chunks = black_box(frame.encode());
            black_box(chunks.len()); // consume so compiler can't optimize away
        });
    }

    macro_rules! bench_data_sizes {
    ($($len:expr),* $(,)?) => {
        $(paste!{
            #[bench] fn [<bench_client_ $len>](b: &mut Bencher) {
                bench_data_frame::<Client>(b, $len);
            }
            #[bench] fn [<bench_server_ $len>](b: &mut Bencher) {
                bench_data_frame::<Server>(b, $len);
            }
        })*
    };
    }

    bench_data_sizes!(125, 1024, 4096, 16384, 32768);
}
