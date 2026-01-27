use std::{
    io::{Result, Write},
    net::TcpStream,
};

use crate::{frame::Opcode, role::Role};

// -- FAST PATH --
// Separate ControlFrame struct to allow a fast path for sending single frames (Ping, Pong, Close) which
// will have a payload <= 125 bytes and FIN always set.
pub(crate) struct ControlFrame<'a> {
    opcode: Opcode,
    payload: &'a [u8],
    role: Role,
}

// Functions to produce each kind of ControlFrame with payload,
// and a send function to write to stream
impl<'a> ControlFrame<'a> {
    pub(crate) fn ping(payload: &'a [u8], role: Role) -> Self {
        Self {
            opcode: Opcode::Ping,
            payload,
            role,
        }
    }

    pub(crate) fn pong(payload: &'a [u8], role: Role) -> Self {
        Self {
            opcode: Opcode::Pong,
            payload,
            role,
        }
    }

    pub(crate) fn close(payload: &'a [u8], role: Role) -> Self {
        Self {
            opcode: Opcode::Close,
            payload,
            role,
        }
    }

    // sets Opcode, FIN, MASK and masks payload
    pub(crate) fn send(self, stream: &mut TcpStream) -> Result<()> {
        let mut buf = [0; 131]; // max single frame size
        buf[0] = self.opcode as u8 | 0x80;
        buf[1] = u8::try_from(self.payload.len()).expect("ControlFrame payload too large") | 0x80;
        // Clients must SEND masked
        if self.role.is_client() {
            rand::fill(&mut buf[2..6]);
            for (i, b) in self.payload.iter().enumerate() {
                buf[6 + i] = b ^ buf[2 + (i % 4)];
            }
            stream.write_all(&buf[..6 + self.payload.len()])
        } else {
            buf[2..2 + self.payload.len()].copy_from_slice(self.payload);
            stream.write_all(&buf[..2 + self.payload.len()])
        }
    }
}
