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
    role: Role,
    current_msg_len: usize,
}

#[derive(Debug)]
enum DecodeState {
    Header1,
    Header2 {
        is_fin: bool,
        opcode: Opcode,
    },
    ExtendedLen {
        is_fin: bool,
        opcode: Opcode,
        payload_len: usize,
        masked: bool,
    },
    Mask {
        is_fin: bool,
        opcode: Opcode,
        payload_len: usize,
    },
    Payload {
        is_fin: bool,
        opcode: Opcode,
        payload_len: usize,
        mask_key: Option<[u8; 4]>,
    },
}

impl FrameDecoder {
    pub(crate) fn new(role: Role) -> Self {
        Self {
            buf: VecDeque::new(),
            state: DecodeState::Header1,
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

                    let Some(opcode) = Opcode::try_from(b & 0x0F).ok() else {
                        return Some(FrameParseResult::ProtoError);
                    };

                    self.state = DecodeState::Header2 {
                        is_fin: b & 0x80 > 0,
                        opcode,
                    };
                }
                DecodeState::Header2 { is_fin, opcode } => {
                    let Some(b) = self.buf.pop_front() else {
                        return Some(FrameParseResult::Incomplete);
                    };

                    let masked = (b & 0x80) != 0;
                    // Servers must NOT mask message
                    if self.role.is_server() != masked {
                        return Some(FrameParseResult::ProtoError);
                    }

                    let payload_len = (b & 0x7F) as usize;
                    // Validate control frame size
                    if matches!(opcode, Opcode::Ping | Opcode::Pong | Opcode::Close)
                        && (!is_fin || payload_len > 125)
                    {
                        return Some(FrameParseResult::ProtoError);
                    }

                    self.state = if payload_len > 125 {
                        DecodeState::ExtendedLen {
                            is_fin,
                            opcode,
                            payload_len,
                            masked,
                        }
                    } else if masked {
                        DecodeState::Mask {
                            is_fin,
                            opcode,
                            payload_len,
                        }
                    } else {
                        DecodeState::Payload {
                            is_fin,
                            opcode,
                            payload_len,
                            mask_key: None,
                        }
                    };
                }
                DecodeState::ExtendedLen {
                    is_fin,
                    opcode,
                    payload_len,
                    masked,
                } => {
                    // 126 => 2 bytes extended
                    // 127 => 8 bytes extended
                    let payload_len = if payload_len == 126 {
                        let Some(len_bytes) = pop_n(&mut self.buf) else {
                            return Some(FrameParseResult::Incomplete);
                        };
                        usize::from(u16::from_be_bytes(len_bytes))
                    } else {
                        let Some(len_bytes) = pop_n(&mut self.buf) else {
                            return Some(FrameParseResult::Incomplete);
                        };

                        let Ok(len) = usize::try_from(u64::from_be_bytes(len_bytes)) else {
                            return Some(FrameParseResult::SizeErr);
                        };

                        len
                    };

                    self.state = if masked {
                        DecodeState::Mask {
                            is_fin,
                            opcode,
                            payload_len,
                        }
                    } else {
                        DecodeState::Payload {
                            is_fin,
                            opcode,
                            payload_len,
                            mask_key: None,
                        }
                    };
                }
                DecodeState::Mask {
                    is_fin,
                    opcode,
                    payload_len,
                } => {
                    let Some(x) = pop_n(&mut self.buf) else {
                        return Some(FrameParseResult::Incomplete);
                    };
                    self.state = DecodeState::Payload {
                        is_fin,
                        opcode,
                        payload_len,
                        mask_key: Some(x),
                    };
                }
                DecodeState::Payload {
                    is_fin,
                    opcode,
                    payload_len,
                    mask_key,
                } => {
                    if self.buf.len() < payload_len {
                        return Some(FrameParseResult::Incomplete);
                    }

                    if self.current_msg_len + payload_len > MAX_MESSAGE_SIZE {
                        return Some(FrameParseResult::SizeErr);
                    }
                    self.current_msg_len += payload_len;

                    let mut payload: Vec<u8> = self.buf.drain(..payload_len).collect();
                    if let Some(mask) = mask_key {
                        payload.iter_mut().enumerate().for_each(|(i, b)| {
                            *b ^= mask[i % 4];
                        });
                    }

                    self.state = DecodeState::Header1;

                    return Some(FrameParseResult::Complete(Frame {
                        opcode,
                        payload,
                        is_fin,
                    }));
                }
            }
        }
    }
}

fn pop_n<const N: usize>(buf: &mut VecDeque<u8>) -> Option<[u8; N]> {
    if N > buf.len() {
        return None;
    }
    Some(array::from_fn(|_| buf.pop_front().unwrap()))
}
