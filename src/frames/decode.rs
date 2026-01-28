use std::collections::VecDeque;

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
    payload_len: usize,
    mask_key: Option<[u8; 4]>,
    opcode: Option<Opcode>,
    is_fin: bool,
    role: Role,
    current_msg_len: usize,
}

#[derive(Debug)]
enum DecodeState {
    Header1,
    Header2,
    ExtendLen(usize),
    Mask,
    Payload,
}

impl FrameDecoder {
    pub(crate) fn new(role: Role) -> Self {
        Self {
            buf: VecDeque::new(),
            state: DecodeState::Header1,
            payload_len: 0,
            mask_key: None,
            opcode: None,
            is_fin: false,
            role,
            current_msg_len: 0,
        }
    }

    pub(crate) fn push_bytes(&mut self, bytes: &[u8]) { self.buf.extend(bytes); }

    pub(crate) fn next_frame(&mut self) -> Option<FrameParseResult> {
        loop {
            match self.state {
                DecodeState::Header1 => {
                    let b = self.buf.pop_front()?;
                    // check RSV bits
                    if b & 0b0111_0000 > 0 {
                        return Some(FrameParseResult::ProtoError);
                    }
                    self.is_fin = b & 0x80 != 0;

                    self.opcode = match Opcode::try_from(b & 0x0F).ok() {
                        Some(opcode) => Some(opcode),
                        None => return Some(FrameParseResult::ProtoError),
                    };

                    self.state = DecodeState::Header2;
                }
                DecodeState::Header2 => {
                    let Some(b) = self.buf.pop_front() else {
                        return Some(FrameParseResult::Incomplete);
                    };

                    let masked = (b & 0x80) != 0;
                    // Servers must NOT mask message
                    if self.role.is_server() != masked {
                        return Some(FrameParseResult::ProtoError);
                    }
                    self.mask_key = if masked { Some([0; 4]) } else { None };

                    let len = (b & 0x7F) as usize;
                    // Validate control frame size
                    if matches!(
                        self.opcode.unwrap(),
                        Opcode::Ping | Opcode::Pong | Opcode::Close
                    ) && (!self.is_fin || len > 125)
                    {
                        return Some(FrameParseResult::ProtoError);
                    }

                    self.state = if len > 125 {
                        DecodeState::ExtendLen(len)
                    } else {
                        self.payload_len = len;
                        if masked {
                            DecodeState::Mask
                        } else {
                            DecodeState::Payload
                        }
                    };
                }
                DecodeState::ExtendLen(len) => {
                    self.payload_len = if len == 126 {
                        // 2 bytes
                        if self.buf.len() < 2 {
                            return Some(FrameParseResult::Incomplete);
                        }

                        usize::from(u16::from_be_bytes([
                            self.buf.pop_front().unwrap(),
                            self.buf.pop_front().unwrap(),
                        ]))
                    } else {
                        // 8 bytes
                        if self.buf.len() < 8 {
                            return Some(FrameParseResult::Incomplete);
                        }

                        let Ok(len) = usize::try_from(u64::from_be_bytes([
                            self.buf.pop_front().unwrap(),
                            self.buf.pop_front().unwrap(),
                            self.buf.pop_front().unwrap(),
                            self.buf.pop_front().unwrap(),
                            self.buf.pop_front().unwrap(),
                            self.buf.pop_front().unwrap(),
                            self.buf.pop_front().unwrap(),
                            self.buf.pop_front().unwrap(),
                        ])) else {
                            return Some(FrameParseResult::SizeErr);
                        };
                        len
                    };

                    self.state = if self.mask_key.is_some() {
                        DecodeState::Mask
                    } else {
                        DecodeState::Payload
                    };
                }
                DecodeState::Mask => {
                    if self.buf.len() < 4 {
                        return Some(FrameParseResult::Incomplete);
                    }

                    self.mask_key = Some([
                        self.buf.pop_front().unwrap(),
                        self.buf.pop_front().unwrap(),
                        self.buf.pop_front().unwrap(),
                        self.buf.pop_front().unwrap(),
                    ]);
                    self.state = DecodeState::Payload;
                }
                DecodeState::Payload => {
                    if self.buf.len() < self.payload_len {
                        return Some(FrameParseResult::Incomplete);
                    }

                    if self.current_msg_len + self.payload_len > MAX_MESSAGE_SIZE {
                        return Some(FrameParseResult::SizeErr);
                    }
                    self.current_msg_len += self.payload_len;

                    let mut payload: Vec<u8> = self.buf.drain(..self.payload_len).collect();
                    if let Some(mask) = self.mask_key {
                        for i in 0..self.payload_len {
                            payload[i] ^= mask[i % 4];
                        }
                    }

                    self.state = DecodeState::Header1;

                    return Some(FrameParseResult::Complete(Frame {
                        opcode: self.opcode.unwrap(),
                        is_fin: self.is_fin,
                        payload,
                    }));
                }
            }
        }
    }
}
