use std::{io::Write, marker::PhantomData};

use bytes::{BufMut, Bytes, BytesMut};
use flate2::write::DeflateEncoder;

use super::Opcode;
use crate::{role::RolePolicy, MAX_FRAME_PAYLOAD, MAX_MESSAGE_SIZE};

// DataFrames may be fragmented or very large hence they need extra processing compared to ControlFrames
#[derive(Debug)]
pub(crate) struct DataFrame<'a, R: RolePolicy> {
    opcode: Opcode,
    payload: &'a [u8],
    _p: PhantomData<R>,
}

impl<'a, R: RolePolicy> DataFrame<'a, R> {
    pub(crate) fn new(payload: &'a [u8], opcode: Opcode) -> Self {
        Self {
            opcode,
            payload,
            _p: PhantomData,
        }
    }

    // None return indicates protocol violation
    pub(crate) fn encode(
        self,
        deflater: &mut Option<DeflateEncoder<Vec<u8>>>,
        use_context: bool,
    ) -> Vec<Bytes> {
        if let Some(deflater) = deflater {
            let init_size = self.payload.len();

            let end = if use_context {
                deflater.get_ref().len()
            } else {
                let _ = deflater.reset(vec![]);
                0
            };

            let _ = deflater.write_all(self.payload);
            let _ = deflater.flush();
            let _ = deflater.flush();

            let b = &deflater.get_ref()[end..];
            tracing::trace!("deflated {init_size} -> {}", b.len());

            self.all_frames(b, true)
        } else {
            self.all_frames(self.payload, false)
        }
    }

    fn all_frames(&self, payload: &[u8], compressed: bool) -> Vec<Bytes> {
        let mut first = true;
        let mut chunks = Vec::with_capacity(MAX_MESSAGE_SIZE.div_ceil(MAX_FRAME_PAYLOAD));

        // TODO: if remainder empty, set last frame properly
        let (chunked, remainder) = payload.as_chunks::<MAX_FRAME_PAYLOAD>();

        for chunk in chunked {
            chunks.push(self.single_frame(chunk, &mut first, false, compressed));
        }
        chunks.push(self.single_frame(remainder, &mut first, true, compressed));

        chunks
    }

    fn single_frame(&self, chunk: &[u8], first: &mut bool, last: bool, compressed: bool) -> Bytes {
        tracing::trace!(
            opcode = ?self.opcode,
            len = chunk.len(),
            first = first,
            fin = last,
            compressed = compressed,
            "encoding DATA"
        );

        // if first, opcode, else CONT
        let mut b1 = if *first { self.opcode } else { Opcode::Cont } as u8;
        // set FIN if last
        b1 |= if last { 0b1000_0000 } else { 0 };
        // set RSV1 if first and compressed
        b1 |= if *first && compressed { 0b0100_0000 } else { 0 };
        // NB: only change once we are done with first
        *first = false;

        let mut buf = BytesMut::with_capacity(chunk.len() + 14);
        buf.put_u8(b1);

        // push LEN
        #[allow(clippy::cast_possible_truncation)]
        match chunk.len() {
            0..=125 => buf.put_u8(chunk.len() as u8),
            126..=65535 => {
                buf.put_u8(126);
                buf.put_u16(chunk.len() as u16);
            }
            _ => {
                buf.put_u8(127);
                buf.put_u64(chunk.len() as u64);
            }
        }

        // Clients must SEND masked
        if R::CLIENT {
            // set MASK bit
            buf[1] |= 0x80;
            // get random bytes and push to buf
            let mut mask_key = [0; 4];
            rand::fill(&mut mask_key);
            buf.extend_from_slice(&mask_key);

            // mask bytes
            let start = buf.len();
            buf.extend_from_slice(chunk);
            crate::protocol::mask(&mut buf[start..], mask_key);
        } else {
            buf.extend_from_slice(chunk);
        }

        buf.freeze()
    }
}

#[cfg(test)]
mod bench {
    extern crate test;
    use paste::paste;
    use test::{black_box, Bencher};

    use super::*;
    use crate::role::*;

    #[allow(clippy::cast_possible_truncation)]
    fn make_payload(len: usize) -> Vec<u8> { (0..len).map(|i| i as u8).collect() }

    fn bench_data_frame<P: RolePolicy>(b: &mut Bencher, payload_len: usize) {
        let payload = make_payload(payload_len);
        b.iter(|| {
            let frame = DataFrame::<P>::new(&payload, Opcode::Text);
            let chunks = black_box(frame.encode(&mut None, false));
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
