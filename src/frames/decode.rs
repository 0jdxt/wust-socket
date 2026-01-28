use std::{array, collections::VecDeque};

use super::{Frame, Opcode};
use crate::role::Role;

const MAX_MESSAGE_SIZE: usize = 1024 * 1024;

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
    current_msg_len: usize,
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
            current_msg_len: 0,
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
                    let Some(x) = pop_n(&mut self.buf) else {
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
            let len_bytes = pop_n(&mut self.buf).ok_or(FrameParseResult::Incomplete)?;
            usize::from(u16::from_be_bytes(len_bytes))
        } else {
            // 127 => 8 bytes extended (u64)
            let len_bytes = pop_n(&mut self.buf).ok_or(FrameParseResult::Incomplete)?;
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

        if self.current_msg_len + self.ctx.payload_len > MAX_MESSAGE_SIZE {
            return Err(FrameParseResult::SizeErr);
        }
        self.current_msg_len += self.ctx.payload_len;

        let mut payload: Vec<u8> = self.buf.drain(..self.ctx.payload_len).collect();
        if let Some(mask) = self.ctx.mask_key {
            payload.iter_mut().enumerate().for_each(|(i, b)| {
                *b ^= mask[i % 4];
            });
        }

        Ok(payload)
    }
}

fn pop_n<const N: usize>(buf: &mut VecDeque<u8>) -> Option<[u8; N]> {
    if N > buf.len() {
        return None;
    }
    Some(array::from_fn(|_| buf.pop_front().unwrap()))
}
