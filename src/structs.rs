use std::io::{Result, Write};
use std::net::TcpStream;

/// This module stores the constants associated with the Close Reasons as specified
pub mod close_reason {
    pub const NORMAL: [u8; 2] = 1000u16.to_be_bytes();
    pub const GOING_AWAY: [u8; 2] = 1001u16.to_be_bytes();
    pub const PROTO_ERROR: [u8; 2] = 1002u16.to_be_bytes();
    pub const ABNORMAL: [u8; 2] = 1006u16.to_be_bytes();
}

#[repr(u8)]
#[derive(Debug, Copy, Clone, PartialEq)]
pub(crate) enum Opcode {
    Cont = 0x0,
    Text = 0x1,
    Bin = 0x2,
    Close = 0x8,
    Ping = 0x9,
    Pong = 0xA,
}

// -- FAST PATH --
// Separate ControlFrame struct to allow a fast path for sending single frames (Ping, Pong, Close) which
// will have a payload <= 125 bytes and FIN always set.
pub(crate) struct ControlFrame<'a> {
    opcode: Opcode,
    payload: &'a [u8],
}

impl<'a> ControlFrame<'a> {
    pub(crate) fn ping(payload: &'a [u8]) -> Self {
        Self {
            opcode: Opcode::Ping,
            payload,
        }
    }

    pub(crate) fn pong(payload: &'a [u8]) -> Self {
        Self {
            opcode: Opcode::Pong,
            payload,
        }
    }

    pub(crate) fn close(payload: &'a [u8]) -> Self {
        Self {
            opcode: Opcode::Close,
            payload,
        }
    }

    pub(crate) fn send(self, stream: &mut TcpStream) -> Result<()> {
        let mut buf = [0; 131];
        buf[0] = self.opcode as u8 | 0x80;
        buf[1] = self.payload.len() as u8 | 0x80;

        rand::fill(&mut buf[2..6]);
        for (i, b) in self.payload.iter().enumerate() {
            let mask = buf[2 + (i % 4)];
            buf[6 + i] = b ^ mask;
        }

        stream.write_all(&buf[..6 + self.payload.len()])
    }
}

// -- SLOW PATH --
// Data Frames may be fragmented or very large hence they need extra processing compared to
// ControlFrames
pub(crate) struct SendMessage<'a> {
    opcode: Opcode,
    payload: &'a [u8],
}

impl<'a> SendMessage<'a> {
    pub(crate) fn new(payload: &'a [u8], opcode: Opcode) -> Self {
        Self { opcode, payload }
    }

    pub(crate) fn send(self, stream: &mut TcpStream) -> Result<()> {
        let mut payload = self.payload;
        let mut first = true;

        while !payload.is_empty() {
            // small chunk example, handle extended lengths later
            let chunk_len = payload.len().min(125);
            let chunk = &payload[..chunk_len];

            let opcode = if first { self.opcode } else { Opcode::Cont };

            let mut buf = vec![0; 6 + chunk_len];
            buf[0] = if chunk_len == payload.len() {
                0x80 | opcode as u8
            } else {
                opcode as u8
            };
            buf[1] = 0x80 | (chunk_len as u8);

            rand::fill(&mut buf[2..6]);
            for (i, &b) in chunk.iter().enumerate() {
                buf[6 + i] = b ^ buf[2 + (i % 4)];
            }

            stream.write_all(&buf)?;

            payload = &payload[chunk_len..];
            first = false;
        }
        Ok(())
    }
}

#[derive(Debug)]
pub(crate) struct Frame {
    pub(crate) opcode: Opcode,
    pub(crate) payload: Vec<u8>,
    pub(crate) is_fin: bool,
}

impl Frame {
    pub(crate) fn parse_bytes(buf: &[u8]) -> impl Iterator<Item = Self> {
        let mut i = 0;
        std::iter::from_fn(move || {
            if i >= buf.len() {
                return None;
            }

            // parse header: FIN, opcode, payload len
            let fin = buf[i] & 0x80 != 0;
            let opcode = match buf[i] & 0x0F {
                0x0 => Opcode::Cont,
                0x1 => Opcode::Text,
                0x2 => Opcode::Bin,
                0x8 => Opcode::Close,
                0x9 => Opcode::Ping,
                0xA => Opcode::Pong,
                _ => return None,
            };
            i += 1;

            // parse length (1 byte, 2 byte extended, or 8 byte extended)
            // TODO: extend for 126/127 values
            let masked = (buf[i] & 0x80) != 0;
            let payload_len = (buf[i] & 0x7F) as usize;
            i += 1;

            let payload = if masked {
                // handle mask
                let mask = &buf[i..i + 4];
                i += 4;
                buf[i..i + payload_len]
                    .iter()
                    .enumerate()
                    .map(|(j, b)| b ^ mask[j % 4])
                    .collect()
            } else {
                buf[i..i + payload_len].to_vec()
            };
            i += payload_len;

            Some(Frame {
                opcode,
                payload,
                is_fin: fin,
            })
        })
    }
}

// TODO: after creating Event, change this to pub(crate)
#[allow(dead_code)]
#[derive(Debug)]
pub struct Message {
    pub(crate) message_type: MessageType,
    pub(crate) payload: Vec<u8>,
}

#[derive(Debug, Copy, Clone)]
pub(crate) enum MessageType {
    Text,
    Binary,
}
