use std::{marker::PhantomData, ops::Deref};

use bytes::{Bytes, BytesMut};

use super::Opcode;
use crate::{role::RolePolicy, MAX_FRAME_PAYLOAD};

// helper type since decoder errors return FrameParseResult
type Result<T> = std::result::Result<T, FrameParseError>;

#[derive(Debug)]
pub(crate) struct DecodedFrame {
    pub(crate) opcode: Opcode,
    pub(crate) payload: Bytes,
    pub(crate) is_fin: bool,
    pub(crate) compressed: bool,
}

#[derive(Debug)]
pub(crate) enum FrameState {
    Complete(DecodedFrame),
    Incomplete,
}

#[derive(Debug)]
pub(crate) enum FrameParseError {
    ProtoError,
    SizeErr,
}

pub(crate) struct FrameDecoder<P: RolePolicy> {
    buf: BytesMut,
    state: DecodeState,
    ctx: DecodeContext,
    compressed: bool,
    _p: PhantomData<P>,
}

#[derive(Debug)]
enum DecodeState {
    Header1,
    Header2,
    ExtendedLen,
    Mask,
    Payload,
}

#[derive(Debug)]
struct DecodeContext {
    is_fin: bool,
    opcode: Opcode,
    payload_len: usize,
    mask_key: [u8; 4],
    compressed: bool,
}

impl<P: RolePolicy> FrameDecoder<P> {
    pub(crate) fn new(compressed: bool) -> Self {
        Self {
            buf: BytesMut::with_capacity(MAX_FRAME_PAYLOAD),
            state: DecodeState::Header1,
            ctx: DecodeContext {
                is_fin: false,
                opcode: Opcode::Cont,
                payload_len: 0,
                mask_key: [0, 0, 0, 0],
                compressed: false,
            },
            compressed,
            _p: PhantomData,
        }
    }

    pub(crate) fn push_bytes(&mut self, bytes: &[u8]) { self.buf.extend_from_slice(bytes); }

    pub(crate) fn next_frame(&mut self) -> Result<Option<FrameState>> {
        tracing::trace!(
            state = ?self.state,
            buf_len = self.buf.len(),
            "decoder"
        );
        loop {
            let next_state = match self.state {
                DecodeState::Header1 => {
                    if self.buf.is_empty() {
                        return Ok(None);
                    }
                    let b = self.buf.split_to(1)[0];
                    self.parse_header1(b)?
                }
                DecodeState::Header2 => match self.parse_header2()? {
                    Some(state) => state,
                    None => return Ok(Some(FrameState::Incomplete)),
                },
                DecodeState::ExtendedLen => match self.parse_extended_len()? {
                    Some(state) => state,
                    None => return Ok(Some(FrameState::Incomplete)),
                },

                DecodeState::Mask => {
                    let Some(x) = self.pop_n() else {
                        return Ok(Some(FrameState::Incomplete));
                    };
                    self.ctx.mask_key = x;
                    DecodeState::Payload
                }
                DecodeState::Payload => {
                    let Some(payload) = self.parse_payload()? else {
                        return Ok(Some(FrameState::Incomplete));
                    };
                    self.state = DecodeState::Header1;

                    tracing::trace!(
                        opcode = ?self.ctx.opcode,
                        fin = self.ctx.is_fin,
                        payload_len = payload.len(),
                        masked = self.ctx.mask_key[0] > 0,
                        "frame decoded"
                    );
                    return Ok(Some(FrameState::Complete(DecodedFrame {
                        opcode: self.ctx.opcode,
                        payload: payload.freeze(),
                        is_fin: self.ctx.is_fin,
                        compressed: self.ctx.compressed,
                    })));
                }
            };
            tracing::trace!(
                from = ?self.state,
                to = ?next_state,
                "state transition"
            );
            self.state = next_state;
        }
    }

    fn parse_header1(&mut self, b: u8) -> Result<DecodeState> {
        // 0   | 1 2 3 | 4 5 6 7
        // Fin | Rsv   | Opcode
        let rsv = b & 0b0011_0000 > 0;
        let compressed = b & 0b0100_0000 > 0;
        if rsv || compressed && !self.compressed {
            tracing::warn!("invalid RSV bits");
            return Err(FrameParseError::ProtoError);
        }

        self.ctx = DecodeContext {
            is_fin: b & 0b1000_0000 > 0,
            compressed,
            opcode: Opcode::try_from(b & 0b1111).map_err(|()| {
                tracing::trace!("invalid opcode");
                FrameParseError::ProtoError
            })?,

            payload_len: 0,
            mask_key: [0, 0, 0, 0],
        };

        Ok(DecodeState::Header2)
    }

    fn parse_header2(&mut self) -> Result<Option<DecodeState>> {
        // 0    | 1 2 3 4 5 6 7
        // Mask | Payload len
        if self.buf.is_empty() {
            return Ok(None);
        }

        let b = self.buf.split_to(1)[0];
        let masked = (b & 0b1000_0000) > 0;
        // Servers must NOT mask message
        if P::SERVER != masked {
            tracing::trace!("message mask violates policy");
            return Err(FrameParseError::ProtoError);
        }

        self.ctx.payload_len = (b & 0b0111_1111) as usize;

        // Validate control frames
        if self.ctx.opcode.is_control()
            // must be FIN, not compressed and max 125B payload
            && (!self.ctx.is_fin || self.ctx.compressed || self.ctx.payload_len > 125)
        {
            tracing::trace!("invalid control frame received");
            return Err(FrameParseError::ProtoError);
        }

        Ok(Some(if self.ctx.payload_len > 125 {
            DecodeState::ExtendedLen
        } else if P::SERVER {
            DecodeState::Mask
        } else {
            DecodeState::Payload
        }))
    }

    fn parse_extended_len(&mut self) -> Result<Option<DecodeState>> {
        self.ctx.payload_len = if self.ctx.payload_len == 126 {
            // 126 => 2 bytes extended (u16)
            let Some(len_bytes) = self.pop_n() else {
                return Ok(None);
            };
            usize::from(u16::from_be_bytes(len_bytes))
        } else {
            // 127 => 8 bytes extended (u64)
            let Some(len_bytes) = self.pop_n() else {
                return Ok(None);
            };
            usize::try_from(u64::from_be_bytes(len_bytes)).map_err(|_| {
                tracing::trace!("frame exceeded maximum size");
                FrameParseError::SizeErr
            })?
        };

        Ok(Some(if P::SERVER {
            DecodeState::Mask
        } else {
            DecodeState::Payload
        }))
    }

    fn parse_payload(&mut self) -> Result<Option<BytesMut>> {
        if self.buf.len() < self.ctx.payload_len {
            return Ok(None);
        }

        if self.ctx.payload_len > MAX_FRAME_PAYLOAD {
            self.buf.clear();
            self.state = DecodeState::Header1;
            tracing::trace!("payload larger than maximum size");
            return Err(FrameParseError::SizeErr);
        }

        let mut payload = self.buf.split_to(self.ctx.payload_len);

        if P::SERVER {
            crate::protocol::mask(&mut payload, self.ctx.mask_key);
        }

        if self.ctx.opcode == Opcode::Close && !is_valid_close_payload(&payload) {
            return Err(FrameParseError::ProtoError);
        }

        Ok(Some(payload))
    }

    fn pop_n<const N: usize>(&mut self) -> Option<[u8; N]> {
        if N > self.buf.len() {
            return None;
        }
        self.buf.split_to(N).deref().try_into().ok()
    }
}

fn is_valid_close_payload(bytes: &[u8]) -> bool {
    match bytes.len() {
        0 => true,
        1 => false,
        _ => {
            let code = u16::from_be_bytes([bytes[0], bytes[1]]);
            matches!(code , 1000..=1011 | 3000..=4999) && str::from_utf8(&bytes[2..]).is_ok()
        }
    }
}

#[cfg(test)]
mod tests {
    use proptest::{
        collection::{vec, VecStrategy},
        num,
        prelude::*,
        strategy::ValueTree,
        test_runner::TestRunner,
    };

    use super::*;
    use crate::role::Client;

    // Strategy to generate a valid opcode
    fn opcode_strategy() -> BoxedStrategy<Opcode> {
        prop_oneof![
            Just(Opcode::Text),
            Just(Opcode::Bin),
            Just(Opcode::Cont),
            Just(Opcode::Ping),
            Just(Opcode::Pong),
            Just(Opcode::Close),
        ]
        .boxed()
    }

    // Strategy to generate a valid WebSocket payload
    fn payload_strategy(opcode: Opcode) -> VecStrategy<num::u8::Any> {
        let max_len = match opcode {
            Opcode::Close | Opcode::Ping | Opcode::Pong => 125, // control frames max 125
            _ => 1024,                                          // arbitrary fuzz max
        };
        vec(any::<u8>(), 0..=max_len)
    }

    // Build a raw WebSocket frame from opcode and payload
    fn build_frame_bytes(opcode: Opcode, payload: &[u8], fin: bool, mask: bool) -> Vec<u8> {
        #![allow(clippy::cast_possible_truncation)]

        let mut bytes = Vec::new();
        let mut b1 = opcode as u8 & 0x0F;
        if fin {
            b1 |= 0x80;
        }
        bytes.push(b1);

        if payload.len() <= 125 {
            let mut b2 = payload.len() as u8;
            if mask {
                b2 |= 0x80;
            }
            bytes.push(b2);
        } else if u16::try_from(payload.len()).is_ok() {
            let mut b2 = 126;
            if mask {
                b2 |= 0x80;
            }
            bytes.push(b2);
            bytes.extend_from_slice(&(payload.len() as u16).to_be_bytes());
        } else {
            let mut b2 = 127;
            if mask {
                b2 |= 0x80;
            }
            bytes.push(b2);
            bytes.extend_from_slice(&(payload.len() as u64).to_be_bytes());
        }

        if mask {
            let mask_key = [0xAA, 0xBB, 0xCC, 0xDD];
            bytes.extend_from_slice(&mask_key);
            for (i, byte) in payload.iter().enumerate() {
                bytes.push(byte ^ mask_key[i % 4]);
            }
        } else {
            bytes.extend_from_slice(payload);
        }

        bytes
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(1000))]

       #[test]
        fn decoder_handles_random_frames(
            opcode in opcode_strategy(),
            fin in any::<bool>(),
        ) {

            let mask = Client::SERVER;
            let payload = payload_strategy(opcode).new_tree(&mut TestRunner::default()).unwrap().current();

            let frame_bytes = build_frame_bytes(opcode, &payload, fin, mask);
            let mut decoder = FrameDecoder::<Client>::new(false);
            decoder.push_bytes(&frame_bytes);

            match decoder.next_frame() {
                Ok(Some(FrameState::Complete(frame))) => {
                    // payload matches original
                    assert_eq!(frame.payload, payload);
                    assert_eq!(frame.opcode, opcode);
                    assert_eq!(frame.is_fin, fin);
                }
                Ok(Some(FrameState::Incomplete)) => panic!("Got Incomplete for full frame"),
                _ => {}
            }
        }

        #[test]
        fn fuzz_decoder(buf in vec(any::<u8>(), 0..2048)) {
            let mut fd = FrameDecoder::<Client>::new(false);
            fd.push_bytes(&buf);

            while let Ok(Some(state)) = fd.next_frame() {
                if let FrameState::Incomplete = state {
                    break;
                }
            }
        }
    }
}

#[cfg(test)]
mod bench {
    #![allow(clippy::cast_possible_truncation)]
    extern crate test;

    use test::{black_box, Bencher};

    use super::*;
    use crate::role::{Client, RolePolicy, Server};

    fn make_test_frame<R: RolePolicy>(payload_len: usize) -> Vec<u8> {
        let mut frame = Vec::with_capacity(2 + payload_len);
        let fin_rsv_opcode = 0b1000_0000; // FIN set, RSV=0, opcode=0 (continuation)
        frame.push(fin_rsv_opcode);

        if payload_len <= 125 {
            let mut second = payload_len as u8;
            if R::SERVER {
                second |= 0b1000_0000;
            }
            frame.push(second);
        } else if payload_len <= 65535 {
            let mut second = 126;
            if R::SERVER {
                second |= 0b1000_0000;
            }
            frame.push(second);
            frame.extend_from_slice(&(payload_len as u16).to_be_bytes());
        } else {
            let mut second = 127;
            if R::SERVER {
                second |= 0b1000_0000;
            }
            frame.push(second);
            frame.extend_from_slice(&(payload_len as u64).to_be_bytes());
        }

        if R::SERVER {
            let mask_key = [1, 2, 3, 4];
            frame.extend_from_slice(&mask_key); // mask key
            for i in 0..payload_len {
                frame.push((i as u8) ^ mask_key[i % 4]); // simple masked payload
            }
        } else {
            frame.extend((0..payload_len).map(|i| i as u8));
        }
        frame
    }

    fn bench_decode_frame<T>(b: &mut Bencher, payload_len: usize)
    where
        T: RolePolicy,
    {
        let frame = make_test_frame::<T>(payload_len);
        let mut decoder = FrameDecoder::<T>::new(false);
        b.iter(|| {
            decoder.push_bytes(black_box(&frame));
            loop {
                if let Ok(f) = decoder.next_frame() {
                    black_box(f);
                    break;
                }
            }
        });
    }

    use paste::paste;

    macro_rules! bench_frame {
    ($($len:expr),* $(,)?) => {
        $(paste! {
            #[bench]
            fn [<bench_client_$len>](b: &mut Bencher) {
                bench_decode_frame::<Client>(b, $len);
            }
            #[bench]
            fn [<bench_server_$len>](b: &mut Bencher) {
                bench_decode_frame::<Server>(b, $len);
            }
        })*
    }
    }

    // Benchmarks for different payloads
    bench_frame!(125, 1024, 4096, 8192, 16384, 32768);
}
