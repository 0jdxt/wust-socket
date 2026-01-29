use std::{array, collections::VecDeque};

use super::{Frame, Opcode};
use crate::{role::Role, MAX_FRAME_PAYLOAD};

#[derive(Debug)]
pub(crate) enum FrameParseResult {
    Complete(Frame),
    Incomplete,
    ProtoError,
    SizeErr,
}

pub(crate) struct FrameDecoder {
    buf: VecDeque<u8>,
    state: DecodeState,
    ctx: DecodeContext,
    role: Role,
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
    mask_key: Option<[u8; 4]>,
    masked: bool,
}

impl FrameDecoder {
    pub(crate) fn new(role: Role) -> Self {
        Self {
            buf: VecDeque::new(),
            state: DecodeState::Header1,
            role,
            ctx: DecodeContext {
                is_fin: false,
                opcode: Opcode::Cont,
                payload_len: 0,
                mask_key: None,
                masked: false,
            },
        }
    }

    pub(crate) fn push_bytes(&mut self, bytes: &[u8]) { self.buf.extend(bytes); }

    pub(crate) fn next_frame(&mut self) -> Option<FrameParseResult> {
        loop {
            let next_state = match self.state {
                DecodeState::Header1 => {
                    let b = self.buf.pop_front()?;
                    match self.parse_header1(b) {
                        Ok(state) => state,
                        Err(e) => return Some(e),
                    }
                }
                DecodeState::Header2 => match self.parse_header2() {
                    Ok(state) => state,
                    Err(e) => {
                        return Some(e);
                    }
                },
                DecodeState::ExtendedLen => match self.parse_extended_len() {
                    Ok(state) => state,
                    Err(e) => return Some(e),
                },
                DecodeState::Mask => {
                    let Some(x) = self.pop_n() else {
                        return Some(FrameParseResult::Incomplete);
                    };
                    self.ctx.mask_key = Some(x);
                    DecodeState::Payload
                }
                DecodeState::Payload => {
                    return match self.parse_payload() {
                        Ok(payload) => {
                            self.state = DecodeState::Header1;
                            Some(FrameParseResult::Complete(Frame {
                                opcode: self.ctx.opcode,
                                payload,
                                is_fin: self.ctx.is_fin,
                            }))
                        }
                        Err(e) => Some(e),
                    };
                }
            };
            // println!("{:?} -> {:?}", self.state, next_state);
            self.state = next_state;
        }
    }

    fn parse_header1(&mut self, b: u8) -> Result<DecodeState, FrameParseResult> {
        // 0   | 1 2 3 | 4 5 6 7
        // Fin | Rsv   | Opcode
        if b & 0b0111_0000 > 0 {
            return Err(FrameParseResult::ProtoError);
        }
        self.ctx = DecodeContext {
            is_fin: b & 0b1000_0000 > 0,
            opcode: Opcode::try_from(b & 0b1111).map_err(|()| FrameParseResult::ProtoError)?,
            payload_len: 0,
            mask_key: None,
            masked: false,
        };
        Ok(DecodeState::Header2)
    }

    fn parse_header2(&mut self) -> Result<DecodeState, FrameParseResult> {
        // 0    | 1 2 3 4 5 6 7
        // Mask | Payload len
        let Some(b) = self.buf.pop_front() else {
            return Err(FrameParseResult::Incomplete);
        };

        self.ctx.masked = (b & 0b1000_0000) > 0;
        // Servers must NOT mask message
        if self.role.is_server() != self.ctx.masked {
            return Err(FrameParseResult::ProtoError);
        }

        self.ctx.payload_len = (b & 0b0111_1111) as usize;
        // Validate control frame size
        if self.ctx.opcode.is_control() && (!self.ctx.is_fin || self.ctx.payload_len > 125) {
            return Err(FrameParseResult::ProtoError);
        }

        Ok(if self.ctx.payload_len > 125 {
            DecodeState::ExtendedLen
        } else if self.ctx.masked {
            DecodeState::Mask
        } else {
            DecodeState::Payload
        })
    }

    fn parse_extended_len(&mut self) -> Result<DecodeState, FrameParseResult> {
        self.ctx.payload_len = if self.ctx.payload_len == 126 {
            // 126 => 2 bytes extended (u16)
            let len_bytes = self.pop_n().ok_or(FrameParseResult::Incomplete)?;
            usize::from(u16::from_be_bytes(len_bytes))
        } else {
            // 127 => 8 bytes extended (u64)
            let len_bytes = self.pop_n().ok_or(FrameParseResult::Incomplete)?;
            usize::try_from(u64::from_be_bytes(len_bytes)).map_err(|_| FrameParseResult::SizeErr)?
        };

        Ok(if self.ctx.masked {
            DecodeState::Mask
        } else {
            DecodeState::Payload
        })
    }

    fn parse_payload(&mut self) -> Result<Vec<u8>, FrameParseResult> {
        if self.buf.len() < self.ctx.payload_len {
            return Err(FrameParseResult::Incomplete);
        }

        // send close and wait for response
        if self.ctx.payload_len > MAX_FRAME_PAYLOAD {
            self.buf.clear();
            self.state = DecodeState::Header1;
            return Err(FrameParseResult::SizeErr);
        }

        let mut payload: Vec<u8> = self.buf.drain(..self.ctx.payload_len).collect();
        if self.ctx.opcode == Opcode::Close && !is_valid_close_payload(&payload) {
            return Err(FrameParseResult::ProtoError);
        }

        if let Some(mask) = self.ctx.mask_key {
            payload.iter_mut().enumerate().for_each(|(i, b)| {
                *b ^= mask[i % 4];
            });
        }

        Ok(payload)
    }

    fn pop_n<const N: usize>(&mut self) -> Option<[u8; N]> {
        if N > self.buf.len() {
            return None;
        }
        Some(array::from_fn(|_| self.buf.pop_front().unwrap()))
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

    fn role_strategy() -> BoxedStrategy<Role> {
        prop_oneof![Just(Role::Client), Just(Role::Server)].boxed()
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
        } else if payload.len() <= u16::MAX as usize {
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
            role in role_strategy(),
        ) {

            let mask = role.is_client();
            let payload = payload_strategy(opcode).new_tree(&mut TestRunner::default()).unwrap().current();

            let frame_bytes = build_frame_bytes(opcode, &payload, fin, mask);
            let mut decoder = FrameDecoder::new(Role::Server);
            decoder.push_bytes(&frame_bytes);

            match decoder.next_frame() {
                Some(FrameParseResult::Complete(frame)) => {
                    // payload matches original
                    assert_eq!(frame.payload, payload);
                    assert_eq!(frame.opcode, opcode);
                    assert_eq!(frame.is_fin, fin);
                }
                Some(FrameParseResult::Incomplete) => panic!("Got Incomplete for full frame"),
                None => panic!("next_frame returned None unexpectedly"),
                _ => {}
            }
        }

        #[test]
        fn fuzz_decoder(buf in vec(any::<u8>(), 0..2048)) {
            let mut fd = FrameDecoder::new(Role::Client);
            fd.push_bytes(&buf);

            while let Some(result) = fd.next_frame() {
                if let FrameParseResult::Incomplete = result {
                    break;
                }
            }
        }
    }
}
